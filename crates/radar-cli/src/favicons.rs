//! `radar favicons`: fetch one small icon per tool into a directory, with a
//! `manifest.json` that says where each came from (or why there is none).
//!
//! Order per tool: manual `favicon` URL, GitHub owner avatar (for github.com
//! URLs), icons declared in the page's HTML, `/favicon.ico`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use url::Url;

use crate::github;
use crate::project::Sources;

pub const MAX_CONCURRENCY: usize = 16;
pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_HTML_BYTES: usize = 1024 * 1024;
pub const MAX_ICON_BYTES: usize = 256 * 1024;
pub const MANIFEST: &str = "manifest.json";
/// Icons of at least this many pixels are preferred over smaller ones.
const MIN_PREFERRED_PX: u32 = 32;

/// One tool's result in `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// File name inside the favicon directory; `None` when nothing was found.
    pub file: Option<String>,
    /// URL the icon came from.
    pub source: Option<String>,
    /// How it was found: `manual`, `github`, `html` or `default`.
    pub kind: Option<String>,
    /// RFC 3339 time of the (last) attempt.
    pub fetched_at: String,
    /// Why nothing was found.
    pub error: Option<String>,
}

pub type Manifest = BTreeMap<String, Entry>;

pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join(MANIFEST);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

#[derive(Clone)]
pub struct Config {
    pub timeout: Duration,
    pub max_concurrency: usize,
    /// Files younger than this are not fetched again.
    pub max_age: Duration,
    /// `https://github.com`; replaced by the mock server in tests.
    pub github_base: String,
}

impl Config {
    pub fn new(max_age_days: u64) -> Self {
        Config {
            timeout: TIMEOUT,
            max_concurrency: MAX_CONCURRENCY,
            max_age: Duration::from_secs(max_age_days * 24 * 3600),
            github_base: "https://github.com".into(),
        }
    }
}

/// `https://github.com/<owner>.png?size=64` for a URL on github.com.
pub fn github_avatar_url(github_base: &str, tool_url: &str) -> Option<String> {
    let url = Url::parse(tool_url).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let owner = url.path_segments()?.find(|s| !s.is_empty())?;
    Some(format!(
        "{}/{owner}.png?size=64",
        github_base.trim_end_matches('/')
    ))
}

/// Biggest side of a `sizes` attribute (`"16x16 32x32"`); `any` counts as huge.
fn parse_sizes(sizes: &str) -> Option<u32> {
    sizes
        .split_whitespace()
        .filter_map(|s| {
            if s.eq_ignore_ascii_case("any") {
                return Some(u32::MAX);
            }
            let (w, h) = s
                .to_ascii_lowercase()
                .split_once('x')
                .map(|(w, h)| (w.parse::<u32>().ok(), h.parse::<u32>().ok()))?;
            Some(w?.max(h?))
        })
        .max()
}

#[derive(Debug)]
struct IconLink {
    url: Url,
    svg: bool,
    /// Declared size in pixels (`None` when `sizes` is missing).
    size: Option<u32>,
    apple: bool,
    png: bool,
}

impl IconLink {
    /// svg, apple-touch-icon, or a png of at least 32px.
    fn preferred(&self) -> bool {
        self.svg || self.apple || (self.png && self.size.is_some_and(|s| s >= MIN_PREFERRED_PX))
    }
}

/// Icon URLs declared in `html`, best first. Relative URLs are resolved
/// against `base`. Preferred: svg, apple-touch-icon, and png of 32px or more
/// (svg first, then the smallest big size); the rest follow in document order.
pub fn pick_candidates(html: &str, base: &Url) -> Vec<Url> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("link[rel][href]").expect("static selector");
    // `<base href>` changes how relative URLs resolve.
    let base = Selector::parse("base[href]")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .and_then(|b| b.value().attr("href"))
        .and_then(|h| base.join(h).ok())
        .unwrap_or_else(|| base.clone());

    let mut links = Vec::new();
    for el in doc.select(&sel) {
        let v = el.value();
        let rel = v.attr("rel").unwrap_or_default().to_ascii_lowercase();
        let tokens: Vec<&str> = rel.split_whitespace().collect();
        let apple = tokens
            .iter()
            .any(|t| *t == "apple-touch-icon" || *t == "apple-touch-icon-precomposed");
        if !apple && !tokens.contains(&"icon") {
            continue;
        }
        let href = v.attr("href").unwrap_or_default().trim();
        if href.is_empty() || href.starts_with("data:") {
            continue;
        }
        let Ok(url) = base.join(href) else { continue };
        if url.scheme() != "http" && url.scheme() != "https" {
            continue;
        }
        let ty = v.attr("type").unwrap_or_default().to_ascii_lowercase();
        let svg = ty.contains("svg") || url.path().to_ascii_lowercase().ends_with(".svg");
        let mut size = v.attr("sizes").and_then(parse_sizes);
        if apple && size.is_none() {
            size = Some(180); // browsers' default for apple-touch-icon
        }
        let png = ty.contains("png") || url.path().to_ascii_lowercase().ends_with(".png");
        links.push(IconLink {
            url,
            png,
            svg,
            size,
            apple,
        });
    }

    let (mut good, rest): (Vec<IconLink>, Vec<IconLink>) =
        links.into_iter().partition(IconLink::preferred);
    good.sort_by_key(|l| (!l.svg, l.size.unwrap_or(u32::MAX)));
    let mut out: Vec<Url> = Vec::new();
    for l in good.into_iter().chain(rest) {
        if !out.contains(&l.url) {
            out.push(l.url);
        }
    }
    out
}

/// Image type recognised from the first bytes, as a file extension.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some("ico")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
        let head = head.trim_start_matches('\u{feff}').trim_start();
        (head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg")))
            .then_some("svg")
    }
}

struct Fetcher {
    client: reqwest::Client,
}

fn describe(e: &reqwest::Error) -> String {
    let mut chain = vec![e.to_string()];
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        chain.push(s.to_string());
        src = s.source();
    }
    let all = chain.join(": ").to_ascii_lowercase();
    if e.is_timeout() {
        "timeout".into()
    } else if all.contains("dns") || all.contains("resolve") || all.contains("lookup") {
        "dns".into()
    } else if e.is_connect() {
        "connect".into()
    } else {
        chain.last().cloned().unwrap_or_default()
    }
}

impl Fetcher {
    /// GET `url`, reading at most `limit` bytes; over the limit, `Err` (or, with
    /// `truncate`, the first `limit` bytes: enough for a page's `<head>`).
    async fn get(
        &self,
        url: &Url,
        limit: usize,
        truncate: bool,
    ) -> Result<(Option<String>, Vec<u8>), String> {
        let mut resp = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| describe(&e))?;
        if !resp.status().is_success() {
            return Err(format!("http {}", resp.status().as_u16()));
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            });
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| describe(&e))? {
            body.extend_from_slice(&chunk);
            if body.len() > limit {
                if truncate {
                    body.truncate(limit);
                    break;
                }
                return Err(format!("larger than {} KB", limit / 1024));
            }
        }
        Ok((ct, body))
    }

    /// A valid icon: `image/*`, recognised magic bytes, at most 256 KB.
    async fn icon(&self, url: &Url) -> Result<(&'static str, Vec<u8>), String> {
        let (ct, body) = self.get(url, MAX_ICON_BYTES, false).await?;
        if !ct.as_deref().is_some_and(|c| c.starts_with("image/")) {
            return Err(format!("not an image ({})", ct.unwrap_or_default()));
        }
        let ext = sniff(&body).ok_or_else(|| "unknown image format".to_string())?;
        Ok((ext, body))
    }

    async fn html(&self, url: &Url) -> Result<(Url, String), String> {
        let (_, body) = self.get(url, MAX_HTML_BYTES, true).await?;
        Ok((url.clone(), String::from_utf8_lossy(&body).into_owned()))
    }
}

struct Found {
    ext: &'static str,
    bytes: Vec<u8>,
    source: Url,
    kind: &'static str,
}

async fn find(
    f: &Fetcher,
    cfg: &Config,
    manual: Option<&str>,
    tool_url: &str,
) -> Result<Found, String> {
    let mut errors: Vec<String> = Vec::new();
    macro_rules! attempt {
        ($kind:expr, $url:expr) => {{
            let (kind, url) = ($kind, $url);
            match f.icon(&url).await {
                Ok((ext, bytes)) => {
                    return Ok(Found {
                        ext,
                        bytes,
                        source: url,
                        kind,
                    });
                }
                Err(e) => errors.push(format!("{kind} {url}: {e}")),
            }
        }};
    }

    if let Some(m) = manual
        && let Ok(u) = Url::parse(m)
    {
        attempt!("manual", u);
    }
    if let Some(a) = github_avatar_url(&cfg.github_base, tool_url)
        && let Ok(u) = Url::parse(&a)
    {
        attempt!("github", u);
    }
    let page = Url::parse(tool_url).map_err(|e| format!("invalid url: {e}"))?;
    match f.html(&page).await {
        Ok((final_url, html)) => {
            for u in pick_candidates(&html, &final_url) {
                attempt!("html", u);
            }
        }
        Err(e) => errors.push(format!("html {page}: {e}")),
    }
    if let Ok(u) = page.join("/favicon.ico") {
        attempt!("default", u);
    }
    Err(errors.join("; "))
}

fn now_string(now: OffsetDateTime) -> String {
    now.format(&Rfc3339).unwrap_or_default()
}

/// Whether `entry` has a file that is still fresh.
fn is_fresh(entry: &Entry, dir: &Path, now: OffsetDateTime, max_age: Duration) -> bool {
    let Some(file) = &entry.file else {
        return false;
    };
    if !dir.join(file).is_file() {
        return false;
    }
    OffsetDateTime::parse(&entry.fetched_at, &Rfc3339)
        .map(|t| now - t < time::Duration::try_from(max_age).unwrap_or(time::Duration::MAX))
        .unwrap_or(false)
}

/// A cached entry is reusable only if it came from the manually specified
/// favicon (when there is one), so editing `favicon = "..."` takes effect at once.
fn manual_matches(manual: Option<&str>, cached_source: Option<&str>) -> bool {
    match (manual, cached_source) {
        (None, _) => true,
        (Some(m), Some(s)) => match (Url::parse(m), Url::parse(s)) {
            (Ok(m), Ok(s)) => m == s,
            _ => false,
        },
        (Some(_), None) => false,
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub fetched: usize,
    pub cached: usize,
    pub failed: usize,
}

/// Fetch the icons of all visible tools into `dir`.
pub async fn fetch_all(
    root: &Path,
    dir: &Path,
    cfg: &Config,
    now: OffsetDateTime,
) -> Result<Report> {
    let sources = Sources::load(root)?;
    let project = sources.validated().map_err(|d| {
        for x in &d {
            eprintln!("{x}");
        }
        anyhow::anyhow!("validation failed; run `radar validate`")
    })?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let old = read_manifest(dir).unwrap_or_default();
    let client = reqwest::Client::builder()
        .user_agent(github::user_agent())
        .timeout(cfg.timeout)
        .build()?;
    let fetcher = Arc::new(Fetcher { client });
    let sem = Arc::new(Semaphore::new(cfg.max_concurrency));

    let mut manifest = Manifest::new();
    let mut report = Report::default();
    let mut set = JoinSet::new();
    for tool in project.tools.iter().filter(|t| !t.file.is_hidden()) {
        let id = tool.id.clone();
        let manual = tool.file.favicon.clone();
        if let Some(e) = old.get(&id)
            && is_fresh(e, dir, now, cfg.max_age)
            && manual_matches(manual.as_deref(), e.source.as_deref())
        {
            manifest.insert(id, e.clone());
            report.cached += 1;
            continue;
        }
        let (fetcher, sem, cfg) = (fetcher.clone(), sem.clone(), cfg.clone());
        let url = tool.file.url.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await;
            let r = find(&fetcher, &cfg, manual.as_deref(), &url).await;
            (id, r)
        });
    }
    while let Some(joined) = set.join_next().await {
        let (id, result) = joined?;
        let entry = match result {
            Ok(found) => {
                // Drop an older file of another extension.
                for e in ["png", "ico", "jpg", "gif", "webp", "svg"] {
                    if e != found.ext {
                        let _ = std::fs::remove_file(dir.join(format!("{id}.{e}")));
                    }
                }
                let file = format!("{id}.{}", found.ext);
                std::fs::write(dir.join(&file), &found.bytes)
                    .with_context(|| format!("write {file}"))?;
                report.fetched += 1;
                Entry {
                    file: Some(file),
                    source: Some(found.source.to_string()),
                    kind: Some(found.kind.into()),
                    fetched_at: now_string(now),
                    error: None,
                }
            }
            Err(error) => {
                report.failed += 1;
                Entry {
                    file: None,
                    source: None,
                    kind: None,
                    fetched_at: now_string(now),
                    error: Some(error),
                }
            }
        };
        manifest.insert(id, entry);
    }
    std::fs::write(
        dir.join(MANIFEST),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )
    .context("write manifest.json")?;
    Ok(report)
}

pub fn run_cli(root: &Path, dir: &Path, max_age_days: u64) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let report = rt.block_on(fetch_all(
        root,
        dir,
        &Config::new(max_age_days),
        OffsetDateTime::now_utc(),
    ))?;
    println!(
        "favicons: {} fetched, {} cached, {} without icon (see {})",
        report.fetched,
        report.cached,
        report.failed,
        dir.join(MANIFEST).display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_bypassed_when_manual_favicon_changes() {
        let m = "https://github.com/BerriAI.png?size=64";
        assert!(manual_matches(
            None,
            Some("https://docs.litellm.ai/img/favicon.ico")
        ));
        assert!(manual_matches(Some(m), Some(m)));
        assert!(!manual_matches(
            Some(m),
            Some("https://docs.litellm.ai/img/favicon.ico")
        ));
        assert!(!manual_matches(Some(m), None));
    }

    fn base() -> Url {
        Url::parse("https://example.com/docs/index.html").unwrap()
    }

    fn picks(html: &str) -> Vec<String> {
        pick_candidates(html, &base())
            .into_iter()
            .map(|u| u.to_string())
            .collect()
    }

    #[test]
    fn sizes_prefer_32px_or_more() {
        let html = r#"<link rel="icon" sizes="16x16" href="/a16.png">
            <link rel="icon" type="image/png" sizes="64x64" href="/a64.png">
            <link rel="icon" type="image/png" sizes="32x32" href="/a32.png">"#;
        assert_eq!(
            picks(html),
            [
                "https://example.com/a32.png",
                "https://example.com/a64.png",
                "https://example.com/a16.png"
            ]
        );
    }

    #[test]
    fn svg_comes_first() {
        let html = r#"<link rel="shortcut icon" sizes="48x48" href="/a.png">
            <link rel="icon" type="image/svg+xml" href="/a.svg">"#;
        assert_eq!(picks(html)[0], "https://example.com/a.svg");
    }

    #[test]
    fn apple_touch_icon_is_preferred_over_small_icons() {
        let html = r#"<link rel="icon" href="/favicon.ico" sizes="16x16">
            <link rel="apple-touch-icon" href="/touch.png">"#;
        assert_eq!(
            picks(html),
            [
                "https://example.com/touch.png",
                "https://example.com/favicon.ico"
            ]
        );
    }

    #[test]
    fn relative_urls_resolve_and_base_href_is_honoured() {
        assert_eq!(
            picks(r#"<link rel="icon" href="img/i.png" sizes="32x32">"#),
            ["https://example.com/docs/img/i.png"]
        );
        assert_eq!(
            picks(r#"<base href="https://cdn.example.net/x/"><link rel="icon" href="i.png">"#),
            ["https://cdn.example.net/x/i.png"]
        );
        assert_eq!(
            picks(r#"<link rel="icon" href="//cdn.example.net/i.png" sizes="32x32">"#),
            ["https://cdn.example.net/i.png"]
        );
    }

    #[test]
    fn ignores_other_links_and_data_urls() {
        let html = r#"<link rel="stylesheet" href="/s.css">
            <link rel="mask-icon" href="/m.svg">
            <link rel="icon" href="data:image/png;base64,AAAA">"#;
        assert!(picks(html).is_empty());
    }

    #[test]
    fn sizes_parsing() {
        assert_eq!(parse_sizes("16x16 32x32"), Some(32));
        assert_eq!(parse_sizes("any"), Some(u32::MAX));
        assert_eq!(parse_sizes("huge"), None);
    }

    #[test]
    fn avatar_url_only_for_github() {
        let b = "https://github.com";
        assert_eq!(
            github_avatar_url(b, "https://github.com/openai/codex").as_deref(),
            Some("https://github.com/openai.png?size=64")
        );
        assert_eq!(
            github_avatar_url(b, "https://example.com/openai/codex"),
            None
        );
        assert_eq!(github_avatar_url(b, "https://github.com/"), None);
    }

    #[test]
    fn sniffs_formats() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 1, 2]), Some("png"));
        assert_eq!(sniff(&[0, 0, 1, 0, 1, 0]), Some("ico"));
        assert_eq!(sniff(b"  <svg xmlns=''/>"), Some("svg"));
        assert_eq!(sniff(b"<?xml version='1.0'?><svg/>"), Some("svg"));
        assert_eq!(sniff(b"<!doctype html><html>"), None);
    }
}
