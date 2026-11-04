//! `radar build`: render the static site into a directory.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use askama::Template;
use radar_core::{
    ALL_PLATFORMS, CATEGORY_FACET, Lang, Platform, Project, RepoMeta, SiteConfig, SiteData,
    SiteTool, WORKS_WITH_FACET, build_site_data, date_string, i18n_keys,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::project::{Sources, load_site_config};

const CSS: &str = include_str!("../../../static/style.css");
/// Brand images copied to `static/brand/`; `icon-256.png` is only for the README.
const BRAND: &[(&str, &[u8])] = &[
    (
        "static/brand/favicon-32.png",
        include_bytes!("../../../static/brand/favicon-32.png"),
    ),
    (
        "static/brand/apple-touch-icon.png",
        include_bytes!("../../../static/brand/apple-touch-icon.png"),
    ),
    (
        "static/brand/logo-48.png",
        include_bytes!("../../../static/brand/logo-48.png"),
    ),
];
const LANGS: [&str; 2] = ["en", "ja"];
/// Keys of `i18n/*.toml` that are handed to the table UI.
const UI_PREFIXES: [&str; 4] = ["table.", "pricing.", "platform.", "ui."];

/// Texts of one `i18n/<lang>.toml`.
pub struct I18n(BTreeMap<String, String>);

impl I18n {
    fn t(&self, key: &str) -> String {
        self.0
            .get(key)
            .cloned()
            .unwrap_or_else(|| format!("[[{key}]]"))
    }
}

/// Everything a page needs to know about where and in which language it is.
pub struct Page {
    pub lang: String,
    base_url: String,
    /// Path below the language root, e.g. `removed/` (empty for the list).
    pub path: String,
    pub title: String,
    pub generated_at: String,
    wasm: bool,
    site: SiteConfig,
    i18n: I18n,
    /// Comment block listing the tags of each facet (for the new-tool template).
    tag_guide: String,
}

impl Page {
    pub fn t(&self, key: &str) -> String {
        self.i18n.t(key)
    }
    /// `Filter by Category` / `Categoryで絞り込む`: the label of a column filter button.
    pub fn filter_by(&self, column_key: &str) -> String {
        self.i18n
            .t("ui.filter_by")
            .replace("{col}", &self.i18n.t(column_key))
    }
    pub fn site_title(&self) -> &str {
        &self.site.title
    }
    pub fn repo(&self) -> &str {
        &self.site.repo
    }
    pub fn other_lang(&self) -> &'static str {
        if self.lang == "ja" { "en" } else { "ja" }
    }
    /// Name of the other language, in that language.
    pub fn other_lang_name(&self) -> String {
        self.t(&format!("lang.{}", self.other_lang()))
    }
    /// Link inside the current language.
    pub fn url(&self, path: &str) -> String {
        self.url_in(&self.lang, path)
    }
    fn url_in(&self, lang: &str, path: &str) -> String {
        let prefix = if lang == "ja" { "ja/" } else { "" };
        format!("{}{prefix}{path}", self.base_url)
    }
    /// The same page in another language.
    pub fn url_for(&self, lang: &str) -> String {
        self.url_in(lang, &self.path)
    }
    /// Absolute URL of the same page in `lang` (origin + base_url + path).
    pub fn abs_url_for(&self, lang: &str) -> String {
        format!(
            "{}{}",
            self.site.origin.trim_end_matches('/'),
            self.url_for(lang)
        )
    }
    /// Absolute URL of this very page.
    pub fn canonical(&self) -> String {
        self.abs_url_for(&self.lang)
    }
    /// `<script type="module">` tag that starts the table, if the WASM was built.
    pub fn wasm_script(&self) -> String {
        if self.wasm {
            format!(
                "<script type=\"module\">import init from \"{}pkg/radar_web.js\"; init();</script>",
                self.base_url
            )
        } else {
            String::new()
        }
    }
    /// Google Tag Manager loader, only on the production origin (empty when `gtm_id` is unset).
    pub fn gtm_script(&self) -> String {
        let Some(id) = self.site.gtm_id.as_deref() else {
            return String::new();
        };
        let origin = serde_json::to_string(self.site.origin.trim_end_matches('/'))
            .unwrap_or_else(|_| "\"\"".into())
            .replace('<', "\\u003c");
        format!(
            "<script>\nif (location.origin === {origin} && !window.radarLangRedirect) {{\n  window.dataLayer = window.dataLayer || [];\n  window.gtag = function(){{dataLayer.push(arguments);}};\n  gtag('js', new Date());\n  var s = document.createElement('script');\n  s.async = true;\n  s.src = 'https://www.googletagmanager.com/gtm.js?id={id}';\n  document.head.appendChild(s);\n}}\n</script>"
        )
    }
    /// Script that sends a visitor who saved the other language to that language's page.
    pub fn lang_redirect_script(&self) -> String {
        let json = |s: &str| {
            serde_json::to_string(s)
                .unwrap_or_else(|_| "\"\"".into())
                .replace('<', "\\u003c")
        };
        let other = json(self.other_lang());
        let target = json(&self.url_for(self.other_lang()));
        format!(
            "<script>\n(function () {{\n  var l;\n  try {{ l = localStorage.getItem('lang'); }} catch (e) {{}}\n  if (l === {other}) {{\n    window.radarLangRedirect = true;\n    location.replace({target} + location.search + location.hash);\n  }}\n}})();\n</script>"
        )
    }
    /// JSON of the UI texts the table needs (safe inside `<script>`).
    pub fn ui_strings_json(&self) -> String {
        let map: BTreeMap<&String, &String> = self
            .i18n
            .0
            .iter()
            .filter(|(k, _)| UI_PREFIXES.iter().any(|p| k.starts_with(p)))
            .collect();
        serde_json::to_string(&map)
            .unwrap_or_else(|_| "{}".into())
            .replace('<', "\\u003c")
    }
    /// Root of the list in this page's language.
    pub fn data_base(&self) -> String {
        self.url("")
    }
    /// Whether the table UI (WASM) was built into this site.
    pub fn has_wasm(&self) -> bool {
        self.wasm
    }
    /// A Lucide icon as an inline `<span class="ic">` (same DOM the WASM UI makes).
    pub fn icon(&self, name: &str) -> String {
        let svg = radar_core::icons::svg(name);
        if svg.is_empty() {
            String::new()
        } else {
            format!("<span class=\"ic\">{svg}</span>")
        }
    }
    /// Inside of `<span class="fav">` for a row (same DOM the WASM UI makes).
    pub fn fav(&self, r: &Row) -> String {
        radar_core::icons::favicon_inner(r.favicon.as_deref(), &r.name, &r.id)
    }
    pub fn css_url(&self) -> String {
        format!("{}static/style.css", self.base_url)
    }
    pub fn static_url(&self, rel: &str) -> String {
        format!("{}static/{}", self.base_url, rel)
    }
    /// GitHub's "new file" screen with a template for `data/tools/`.
    pub fn add_tool_url(&self) -> String {
        add_tool_url(
            &self.site.repo,
            date_part(&self.generated_at),
            &self.tag_guide,
        )
    }
    /// GitHub's issue form for suggesting a tool.
    pub fn suggest_tool_url(&self) -> String {
        format!(
            "https://github.com/{}/issues/new?template=suggest-tool.yml",
            self.site.repo
        )
    }
}

/// Template of a new tool file. `added` is the build date; `tag_guide` is the
/// comment block from [`radar_core::guide::tag_guide`].
pub fn add_tool_template(added: &str, tag_guide: &str) -> String {
    format!(
        "# Rename this file to the tool's id (lowercase kebab-case), e.g. my-tool.toml\n\
         # ファイル名をツールの id（小文字ケバブケース）に変えてください。例: my-tool.toml\n\
         name = \"\"\n\
         url = \"\"\n\
         repo = \"\"\n\
         summary.en = \"\"\n\
         summary.ja = \"\"\n\
         # free = no cost, freemium = free tier plus paid plans, paid = payment required\n\
         pricing = \"free\"\n\
         open_source = true\n\
         # macos, windows, linux, web, ios, android\n\
         platforms = []\n\
         {tag_guide}\
         tags = []\n\
         added = {added}\n"
    )
}

pub fn add_tool_url(repo: &str, added: String, tag_guide: &str) -> String {
    format!(
        "https://github.com/{repo}/new/main/data/tools?filename=your-tool-id.toml&value={}",
        percent_encode(&add_tool_template(&added, tag_guide))
    )
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub struct Badge {
    pub label: String,
    pub path: String,
    /// Lucide icon name, empty for none.
    pub icon: String,
}

/// One tile of a row's "Works with" or "Platforms" cell.
pub struct Mark {
    /// Text on the tile, e.g. "Claude" or "macOS".
    pub text: String,
    /// `?tag=works-with-codex` or `?platform=macos`.
    pub path: String,
    /// Label when the tool is listed for it, else "… (not listed)".
    pub title: String,
    /// Whether the tool is listed for it (filled tile) or not (outlined tile).
    pub on: bool,
}

pub struct Row {
    pub id: String,
    /// Path of the favicon (with base URL), if one was fetched.
    pub favicon: Option<String>,
    pub name: String,
    /// Official site.
    pub url: String,
    /// `owner/name` on GitHub, if the tool has a repository.
    pub repo: Option<String>,
    pub summary: String,
    pub categories: Vec<Badge>,
    /// Every tag of the `works-with` facet in the facet's order, with whether the tool has it;
    /// `None` when the tool has none of them (shown as "—").
    pub works_with: Option<Vec<Mark>>,
    /// The six platforms in `ALL_PLATFORMS` order, without `unspecified`;
    /// `None` when no platform is listed (shown as "Not stated").
    pub platforms: Option<Vec<Mark>>,
    pub pricing: String,
    /// `154k`; empty when unknown.
    pub stars: String,
    /// Full number for the tooltip.
    pub stars_full: String,
    /// `3d ago`; empty when unknown.
    pub updated: String,
    /// `YYYY-MM-DD`; empty when unknown.
    pub updated_title: String,
}

pub struct RemovedItem {
    pub name: String,
    pub url: String,
    pub reason: String,
    pub since: String,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage<'a> {
    page: &'a Page,
    interactive: bool,
    rows: Vec<Row>,
    count: String,
    site_json: &'a str,
}

#[derive(Template)]
#[template(path = "removed.html")]
struct RemovedPage<'a> {
    page: &'a Page,
    items: Vec<RemovedItem>,
}

fn normalize_base(b: &str) -> String {
    let mut s = b.trim().to_string();
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if entry.file_name() != ".gitignore" {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn write(out: &Path, rel: &str, content: impl AsRef<[u8]>) -> Result<()> {
    let path = out.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

/// Escape `<` so the JSON cannot close its `<script>` element.
fn json_for_script(data: &SiteData) -> Result<String> {
    Ok(serde_json::to_string(data)?.replace('<', "\\u003c"))
}

/// Returns `Ok(true)` when validation failed (nothing is built then).
pub fn run(
    root: &Path,
    out: &Path,
    base_url: Option<&str>,
    enrich: Option<&Path>,
    favicons: Option<&Path>,
) -> Result<bool> {
    let sources = Sources::load(root)?;
    let (diags, project) = sources.validate();
    let mut failed = false;
    for d in &diags {
        if d.severity == radar_core::Severity::Error {
            println!("{d}");
            failed = true;
        }
    }
    if failed {
        eprintln!("validation failed; site not built");
        return Ok(true);
    }
    let site = load_site_config(root)?;
    let base = normalize_base(base_url.unwrap_or(&site.base_url));
    let enrich_map: BTreeMap<String, RepoMeta> = match enrich {
        Some(p) => serde_json::from_str(
            &std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?,
        )
        .with_context(|| format!("parse {}", p.display()))?,
        None => BTreeMap::new(),
    };
    let generated_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let generated_at = generated_at
        .split('.')
        .next()
        .unwrap_or(&generated_at)
        .to_string()
        + "Z";
    let mut data = build_site_data(
        &project.taxonomy,
        &project.tools,
        &enrich_map,
        generated_at.clone(),
    );
    // Icons of `radar favicons` that exist on disk; copied after `out` is cleaned.
    let mut favicon_files: Vec<(std::path::PathBuf, String)> = Vec::new();
    if let Some(dir) = favicons {
        let manifest = crate::favicons::read_manifest(dir)?;
        for tool in &mut data.tools {
            let file = manifest.get(&tool.id).and_then(|e| e.file.as_ref());
            if let Some(file) = file
                && dir.join(file).is_file()
            {
                tool.favicon = Some(format!("{base}favicons/{file}"));
                favicon_files.push((dir.join(file), file.clone()));
            }
        }
    }
    let site_json = json_for_script(&data)?;

    if out.exists() {
        std::fs::remove_dir_all(out).with_context(|| format!("clean {}", out.display()))?;
    }
    std::fs::create_dir_all(out)?;
    write(out, ".nojekyll", "")?;
    write(out, "static/style.css", CSS)?;
    for (rel, bytes) in BRAND {
        write(out, rel, bytes)?;
    }
    // The table UI (built by scripts/build-site.sh); without it the static
    // table is all there is.
    let pkg = root.join("crates/radar-web/pkg");
    let wasm = pkg.join("radar_web.js").exists();
    if wasm {
        copy_dir(&pkg, &out.join("pkg"))?;
    }
    for (from, name) in &favicon_files {
        std::fs::create_dir_all(out.join("favicons"))?;
        std::fs::copy(from, out.join("favicons").join(name))
            .with_context(|| format!("copy {}", from.display()))?;
    }
    write(out, "tools.json", &serde_json::to_string_pretty(&data)?)?;

    let tag_guide = radar_core::guide::tag_guide(&project.taxonomy);
    let en = i18n_keys(&sources.en.text)?;
    let ja = i18n_keys(&sources.ja.text)?;
    for lang in LANGS {
        let make_page = |path: &str, title: String| Page {
            lang: lang.to_string(),
            base_url: base.clone(),
            path: path.to_string(),
            title,
            generated_at: generated_at.clone(),
            wasm,
            site: site.clone(),
            i18n: I18n(if lang == "ja" { ja.clone() } else { en.clone() }),
            tag_guide: tag_guide.clone(),
        };
        let prefix = if lang == "ja" { "ja/" } else { "" };
        let ctx = Ctx {
            lang,
            data: &data,
            project: &project,
        };

        let page = make_page("", site.title.clone());
        let html = IndexPage {
            page: &page,
            interactive: true,
            rows: ctx.rows(&page, |_| true),
            count: page
                .t("ui.count")
                .replace("{shown}", &data.tools.len().to_string())
                .replace("{total}", &data.tools.len().to_string()),
            site_json: &site_json,
        }
        .render()?;
        write(out, &format!("{prefix}index.html"), &html)?;

        let page = make_page(
            "removed/",
            format!("{} - {}", page.t("removed.title"), site.title),
        );
        let items = project
            .tools
            .iter()
            .filter(|t| t.file.is_hidden())
            .map(|t| RemovedItem {
                name: t.file.name.clone(),
                url: t.file.url.clone(),
                reason: t.file.hidden_reason.clone().unwrap_or_default(),
                since: t
                    .file
                    .hidden_since
                    .as_ref()
                    .map(date_string)
                    .unwrap_or_default(),
            })
            .collect();
        let html = RemovedPage { page: &page, items }.render()?;
        write(out, &format!("{prefix}removed/index.html"), &html)?;
    }

    println!(
        "built {} tools into {} (base {base})",
        data.tools.len(),
        out.display()
    );
    if data.tools.is_empty() {
        bail!("no tools to build");
    }
    Ok(false)
}

/// `YYYY-MM-DD` of a timestamp.
fn date_part(s: &str) -> String {
    s.chars().take(10).collect()
}

struct Ctx<'a> {
    lang: &'a str,
    data: &'a SiteData,
    project: &'a Project,
}

impl Ctx<'_> {
    fn tag_label(&self, id: &str) -> Lang {
        self.data
            .facets
            .iter()
            .flat_map(|f| f.tags.iter())
            .find(|t| t.id == id)
            .map(|t| t.label.clone())
            .unwrap_or(Lang {
                en: id.into(),
                ja: id.into(),
            })
    }

    fn tag_icon(&self, id: &str) -> String {
        self.data
            .facets
            .iter()
            .flat_map(|f| f.tags.iter())
            .find(|t| t.id == id)
            .and_then(|t| t.icon.clone())
            .unwrap_or_default()
    }

    fn row(&self, page: &Page, t: &SiteTool) -> Row {
        let category_ids: Vec<&str> = self
            .data
            .facets
            .iter()
            .filter(|f| f.id == CATEGORY_FACET)
            .flat_map(|f| f.tags.iter().map(|t| t.id.as_str()))
            .collect();
        let badge = |id: &String| Badge {
            label: self.tag_label(id).get(self.lang).to_string(),
            path: format!("?tag={id}"),
            icon: self.tag_icon(id),
        };
        let categories = t
            .tags
            .iter()
            .filter(|id| category_ids.contains(&id.as_str()));
        let works: Vec<Mark> = self
            .data
            .facets
            .iter()
            .filter(|f| f.id == WORKS_WITH_FACET)
            .flat_map(|f| f.tags.iter())
            .map(|tag| {
                let on = t.tags.contains(&tag.id);
                let label = tag.label.get(self.lang).to_string();
                Mark {
                    text: tag.short.get(self.lang).to_string(),
                    path: format!("?tag={}", tag.id),
                    title: if on {
                        label
                    } else {
                        page.t("ui.not_listed").replace("{name}", &label)
                    },
                    on,
                }
            })
            .collect();
        let works_with = works.iter().any(|m| m.on).then_some(works);
        let cells: Vec<Mark> = ALL_PLATFORMS
            .into_iter()
            .filter(|p| *p != Platform::Unspecified)
            .map(|p| {
                let on = t.platforms.contains(&p);
                let label = page.t(&format!("platform.{}", p.as_str()));
                Mark {
                    text: label.clone(),
                    path: format!("?platform={}", p.as_str()),
                    title: if on {
                        label
                    } else {
                        page.t("ui.not_listed").replace("{name}", &label)
                    },
                    on,
                }
            })
            .collect();
        let platforms = cells.iter().any(|c| c.on).then_some(cells);
        Row {
            id: t.id.clone(),
            favicon: t.favicon.clone(),
            name: t.name.clone(),
            url: t.url.clone(),
            repo: t.repo.clone(),
            summary: t.summary.get(self.lang).to_string(),
            categories: categories.map(badge).collect(),
            works_with,
            platforms,
            pricing: page.t(&format!("pricing.{}", t.pricing.as_str())),
            stars: t
                .stars
                .map(radar_core::icons::short_count)
                .unwrap_or_default(),
            stars_full: t.stars.map(|s| s.to_string()).unwrap_or_default(),
            updated: t
                .pushed_at
                .as_deref()
                .and_then(|p| {
                    radar_core::icons::relative_time(p, &self.data.generated_at, self.lang)
                })
                .unwrap_or_default(),
            updated_title: t.pushed_at.as_deref().map(date_part).unwrap_or_default(),
        }
    }

    fn rows(&self, page: &Page, keep: impl Fn(&SiteTool) -> bool) -> Vec<Row> {
        let _ = self.project;
        self.data
            .tools
            .iter()
            .filter(|t| keep(t))
            .map(|t| self.row(page, t))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn add_tool_url_decodes_to_a_readable_toml_template() {
        let tax = radar_core::Taxonomy::parse(
            "[[facet]]\nid = \"category\"\nlabel = { en = \"C\", ja = \"C\" }\nmin = 1\nmax = 3\n\
             [[facet.tag]]\nid = \"sandbox\"\nlabel = { en = \"S\", ja = \"S\" }\n",
        )
        .unwrap();
        let guide = radar_core::guide::tag_guide(&tax);
        let url = add_tool_url("o/r", "2026-11-02".into(), &guide);
        let (head, value) = url.split_once("&value=").unwrap();
        assert_eq!(
            head,
            "https://github.com/o/r/new/main/data/tools?filename=your-tool-id.toml"
        );
        let text = percent_decode(value);
        assert!(text.contains("ファイル名をツールの id"), "{text}");
        assert!(text.contains("Rename this file to the tool's id"), "{text}");
        assert!(
            text.contains("#   category (1 to 3, required): sandbox"),
            "{text}"
        );
        assert!(text.contains("free = no cost"), "{text}");
        let doc: toml::Table = toml::from_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(doc["added"].to_string(), "2026-11-02");
        assert_eq!(doc["pricing"].as_str(), Some("free"));
        assert!(doc["tags"].as_array().unwrap().is_empty());
        assert!(doc["platforms"].as_array().unwrap().is_empty());
        assert_eq!(doc["summary"]["ja"].as_str(), Some(""));
        // The comment sits right above `tags`.
        assert!(text.find("# Tags:").unwrap() < text.find("tags = []").unwrap());
    }
}
