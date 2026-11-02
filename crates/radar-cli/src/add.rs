//! `radar add <URL>`: write a draft `data/tools/<id>.toml` from a GitHub
//! repository or a web page. The draft is meant to fail `radar validate`
//! until a person fills in the Japanese summary, tags and platforms.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use radar_core::guide::tag_guide;
use radar_core::{Taxonomy, Tool, is_slug};
use scraper::{Html, Selector};
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

use crate::github;
use crate::project::Sources;
use crate::tool_file;

pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_HTML_BYTES: usize = 1024 * 1024;

/// SPDX ids (OSI approved, widely used) that make `open_source = true`.
/// `-only` / `-or-later` / `+` suffixes are ignored when matching.
pub const OSI_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "0BSD",
    "GPL-2.0",
    "GPL-3.0",
    "LGPL-2.0",
    "LGPL-2.1",
    "LGPL-3.0",
    "AGPL-3.0",
    "MPL-2.0",
    "ISC",
    "Unlicense",
    "Zlib",
    "BSL-1.0",
    "EPL-1.0",
    "EPL-2.0",
    "EUPL-1.2",
    "Artistic-2.0",
];

pub fn is_osi_license(spdx: &str) -> bool {
    let id = spdx
        .trim_end_matches('+')
        .trim_end_matches("-or-later")
        .trim_end_matches("-only");
    OSI_LICENSES.iter().any(|l| l.eq_ignore_ascii_case(id))
}

#[derive(Clone)]
pub struct Config {
    pub timeout: Duration,
    /// `https://api.github.com`; replaced by the mock server in tests.
    pub github_api: String,
    pub github_token: Option<String>,
    /// `YYYY-MM-DD` written to `added`.
    pub today: String,
}

impl Config {
    pub fn new() -> Self {
        let d = OffsetDateTime::now_utc().date();
        Config {
            timeout: TIMEOUT,
            github_api: github::API_BASE.into(),
            github_token: std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()),
            today: format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day()),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// A made draft; nothing has been written yet.
#[derive(Debug)]
pub struct Draft {
    pub id: String,
    /// Path relative to the repository root.
    pub path: String,
    pub text: String,
    pub warnings: Vec<String>,
}

/// What GitHub knows about a repository.
#[derive(Debug, Default)]
struct RepoInfo {
    /// `owner/name` as GitHub spells it.
    full_name: String,
    name: String,
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    archived: bool,
}

/// `(owner, name)` of `https://github.com/<owner>/<name>[/...]`.
pub fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let mut seg = u.path_segments()?.filter(|s| !s.is_empty());
    let owner = seg.next()?;
    let name = seg.next()?.trim_end_matches(".git");
    let valid = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    // Pages of GitHub itself, not repositories.
    const RESERVED: &[&str] = &[
        "sponsors",
        "orgs",
        "features",
        "topics",
        "settings",
        "login",
        "about",
        "pricing",
        "marketplace",
        "apps",
        "enterprise",
        "collections",
        "search",
        "notifications",
        "explore",
        "users",
        "organizations",
        "site",
        "readme",
        "security",
        "customer-stories",
        "resources",
        "solutions",
        "trending",
    ];
    (valid(owner) && valid(name) && !RESERVED.contains(&owner.to_ascii_lowercase().as_str()))
        .then(|| (owner.to_string(), name.to_string()))
}

/// `claude-code` -> `Claude Code`; mixed-case names (`OpenHands`) stay as they are.
pub fn display_name(repo_name: &str) -> String {
    repo_name
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            if w.chars().any(|c| c.is_ascii_uppercase()) {
                w.to_string()
            } else {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lowercase kebab-case id of a name (`Claude Code` -> `claude-code`).
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn norm_url(u: &str) -> String {
    u.trim().to_lowercase().trim_end_matches('/').to_string()
}

/// The existing tool (and its file) that already has this url or repo.
fn find_duplicate(
    tools: &[(String, Tool)],
    url: Option<&str>,
    repo: Option<&str>,
) -> Option<(String, &'static str)> {
    for (path, t) in tools {
        if let Some(u) = url
            && norm_url(u) == norm_url(&t.file.url)
        {
            return Some((path.clone(), "url"));
        }
        if let (Some(r), Some(tr)) = (repo, &t.file.repo)
            && r.eq_ignore_ascii_case(tr)
        {
            return Some((path.clone(), "repo"));
        }
    }
    None
}

struct Client {
    http: reqwest::Client,
    cfg: Config,
}

impl Client {
    fn new(cfg: &Config) -> Result<Self> {
        Ok(Client {
            http: reqwest::Client::builder()
                .user_agent(github::user_agent())
                .timeout(cfg.timeout)
                .build()?,
            cfg: cfg.clone(),
        })
    }

    async fn repo_info(&self, owner: &str, name: &str) -> Result<RepoInfo> {
        let url = format!(
            "{}/repos/{owner}/{name}",
            self.cfg.github_api.trim_end_matches('/')
        );
        let mut req = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json");
        if let Some(t) = &self.cfg.github_token {
            req = req.bearer_auth(t);
        }
        let res = req
            .send()
            .await
            .with_context(|| format!("GitHub request for {owner}/{name} failed"))?;
        let status = res.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!("repository {owner}/{name} was not found on GitHub");
        }
        if !status.is_success() {
            let hint = if status == reqwest::StatusCode::FORBIDDEN
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                " (rate limit? set GITHUB_TOKEN)"
            } else {
                ""
            };
            bail!("GitHub returned {status} for {owner}/{name}{hint}");
        }
        let v: Value = res.json().await.context("GitHub response is not JSON")?;
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Ok(RepoInfo {
            full_name: text("full_name").unwrap_or_else(|| format!("{owner}/{name}")),
            name: text("name").unwrap_or_else(|| name.to_string()),
            description: text("description"),
            homepage: text("homepage"),
            license: v
                .get("license")
                .and_then(|l| l.get("spdx_id"))
                .and_then(Value::as_str)
                .filter(|s| !matches!(*s, "NOASSERTION" | "NONE" | "Other" | ""))
                .map(str::to_string),
            archived: v.get("archived").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// First [`MAX_HTML_BYTES`] of a page.
    async fn html(&self, url: &Url) -> Result<String> {
        let mut resp = self
            .http
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("cannot fetch {url}"))?;
        if !resp.status().is_success() {
            bail!("{url} returned {}", resp.status());
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_HTML_BYTES {
                body.truncate(MAX_HTML_BYTES);
                break;
            }
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

/// What a page says about itself.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PageInfo {
    pub name: Option<String>,
    pub description: Option<String>,
    /// `owner/name` when the page links to exactly one GitHub repository.
    pub repo: Option<String>,
}

fn collapse(s: &str) -> Option<String> {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// `Foo - the thing` -> `Foo` (the part before the first separator).
fn site_name_from_title(title: &str) -> String {
    for sep in [" | ", " - ", " – ", " — ", " · ", ": "] {
        if let Some((head, _)) = title.split_once(sep)
            && !head.trim().is_empty()
        {
            return head.trim().to_string();
        }
    }
    title.trim().to_string()
}

pub fn parse_page(html: &str) -> PageInfo {
    let doc = Html::parse_document(html);
    let sel = |s: &str| Selector::parse(s).expect("static selector");
    let meta = |attr: &str, val: &str| -> Option<String> {
        doc.select(&sel(&format!("meta[{attr}=\"{val}\"]")))
            .find_map(|m| m.value().attr("content").and_then(collapse))
    };
    let name = meta("property", "og:site_name").or_else(|| {
        doc.select(&sel("title"))
            .next()
            .and_then(|t| collapse(&t.text().collect::<String>()))
            .map(|t| site_name_from_title(&t))
    });
    let description = meta("property", "og:description").or_else(|| meta("name", "description"));
    let mut repos: Vec<(String, String)> = Vec::new();
    for a in doc.select(&sel("a[href]")) {
        if let Some((o, n)) = a.value().attr("href").and_then(parse_github_repo)
            && !repos
                .iter()
                .any(|(po, pn)| po.eq_ignore_ascii_case(&o) && pn.eq_ignore_ascii_case(&n))
        {
            repos.push((o, n));
        }
    }
    let repo = (repos.len() == 1).then(|| format!("{}/{}", repos[0].0, repos[0].1));
    PageInfo {
        name,
        description,
        repo,
    }
}

fn q(s: &str) -> String {
    toml_edit::Value::from(s).to_string().trim().to_string()
}

struct Fields {
    name: String,
    url: String,
    repo: Option<String>,
    summary_en: String,
    open_source: bool,
}

fn render(f: &Fields, tax: &Taxonomy, today: &str) -> Result<String> {
    let mut t = String::new();
    t.push_str(
        "# Draft made by `radar add`. Fill in summary.ja, platforms and tags, check pricing,\n\
         # then run `radar validate`.\n",
    );
    t.push_str(&format!("name = {}\n", q(&f.name)));
    t.push_str(&format!("url = {}\n", q(&f.url)));
    if let Some(r) = &f.repo {
        t.push_str(&format!("repo = {}\n", q(r)));
    }
    t.push_str(&format!("summary.en = {}\n", q(&f.summary_en)));
    t.push_str("summary.ja = \"\"\n");
    t.push_str("# free = no cost, freemium = free tier plus paid plans, paid = payment required\n");
    t.push_str("pricing = \"free\"\n");
    t.push_str(&format!("open_source = {}\n", f.open_source));
    t.push_str("# macos, windows, linux, web, ios, android\nplatforms = []\n");
    t.push_str(&tag_guide(tax));
    t.push_str("tags = []\n");
    t.push_str(&format!("added = {today}\n"));
    tool_file::canonical_text(&t)
}

/// Make the draft for `input` (no file is written).
pub async fn draft(root: &Path, input: &str, id: Option<&str>, cfg: &Config) -> Result<Draft> {
    let parsed = Url::parse(input.trim()).with_context(|| format!("\"{input}\" is not a URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("\"{input}\" is not an http(s) URL");
    }
    let sources = Sources::load(root)?;
    let (_, project) = sources.validate();
    let existing: Vec<(String, Tool)> = project
        .tools
        .iter()
        .map(|t| (project.tool_paths[&t.id].clone(), t.clone()))
        .collect();
    let dup = |url: Option<&str>, repo: Option<&str>| -> Result<()> {
        if let Some((path, what)) = find_duplicate(&existing, url, repo) {
            let v = if what == "url" { url } else { repo }.unwrap_or_default();
            bail!("{what} \"{v}\" is already used by {path}");
        }
        Ok(())
    };

    let client = Client::new(cfg)?;
    let mut warnings = Vec::new();
    let input_repo = parse_github_repo(input);
    let fields = if let Some((owner, name)) = input_repo {
        dup(None, Some(&format!("{owner}/{name}")))?;
        let info = client.repo_info(&owner, &name).await?;
        dup(None, Some(&info.full_name))?;
        let url = info
            .homepage
            .clone()
            .filter(|h| Url::parse(h).is_ok_and(|u| matches!(u.scheme(), "http" | "https")))
            .unwrap_or_else(|| format!("https://github.com/{}", info.full_name));
        dup(Some(&url), None)?;
        if info.archived {
            warnings.push(format!(
                "{} is archived on GitHub; it will probably not stay in the list",
                info.full_name
            ));
        }
        Fields {
            name: display_name(&info.name),
            url,
            repo: Some(info.full_name.clone()),
            summary_en: info.description.clone().unwrap_or_default(),
            open_source: info.license.as_deref().is_some_and(is_osi_license),
        }
    } else {
        dup(Some(input), None)?;
        let html = client.html(&parsed).await?;
        let page = parse_page(&html);
        let mut fields = Fields {
            name: page.name.clone().unwrap_or_default(),
            url: input.trim().to_string(),
            repo: None,
            summary_en: page.description.clone().unwrap_or_default(),
            open_source: false,
        };
        if let Some(repo) = &page.repo {
            let (o, n) = repo.split_once('/').expect("owner/name");
            dup(None, Some(repo))?;
            match client.repo_info(o, n).await {
                Ok(info) => {
                    dup(None, Some(&info.full_name))?;
                    if info.archived {
                        warnings.push(format!("{} is archived on GitHub", info.full_name));
                    }
                    fields.repo = Some(info.full_name.clone());
                    fields.open_source = info.license.as_deref().is_some_and(is_osi_license);
                    if fields.name.is_empty() {
                        fields.name = display_name(&info.name);
                    }
                    if fields.summary_en.is_empty() {
                        fields.summary_en = info.description.unwrap_or_default();
                    }
                }
                Err(e) => {
                    fields.repo = Some(repo.clone());
                    warnings.push(format!("repo {repo}: {e:#}; open_source left false"));
                }
            }
        } else {
            warnings.push("no single GitHub repository found on the page; repo left out".into());
        }
        fields
    };

    if fields.name.is_empty() && id.is_none() {
        bail!("could not find a name on the page; pass --id <id> and fill in name by hand");
    }
    let id = match id {
        Some(i) => i.to_string(),
        None => slugify(&fields.name),
    };
    if !is_slug(&id) {
        bail!(
            "id \"{id}\" must be lowercase letters, digits and '-'; pass --id <id> to choose one"
        );
    }
    let rel = format!("data/tools/{id}.toml");
    if root.join(&rel).exists() {
        bail!("{rel} already exists");
    }
    let text = render(&fields, &project.taxonomy, &cfg.today)?;
    Ok(Draft {
        id,
        path: rel,
        text,
        warnings,
    })
}

/// `radar add`.
pub fn run_cli(root: &Path, url: &str, id: Option<&str>, dry_run: bool) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let d = rt.block_on(draft(root, url, id, &Config::new()))?;
    for w in &d.warnings {
        eprintln!("warning: {w}");
    }
    if dry_run {
        println!("# {} (dry run: nothing written)", d.path);
        print!("{}", d.text);
        return Ok(());
    }
    let path: PathBuf = root.join(&d.path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // `create_new` so a file made in the meantime is never overwritten.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("{} already exists or cannot be created", d.path))?;
        f.write_all(d.text.as_bytes())?;
    }
    println!("created {}", d.path);
    println!("next:");
    println!("  - fill in summary.ja (Japanese summary)");
    println!("  - fill in tags (at least one category; see the comment above `tags`)");
    println!("  - fill in platforms");
    println!("  - check pricing (free / freemium / paid) and open_source");
    println!("then run: cargo run -p radar-cli -- validate");
    Ok(())
}
