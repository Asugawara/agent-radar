use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::model::{Facet, RawLang, Taxonomy, Tool, i18n_keys};
use crate::site::date_string;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One finding, printed as `path: message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: String,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    fn error(path: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            path: path.to_string(),
            severity: Severity::Error,
            message: message.into(),
        }
    }
    fn warning(path: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            path: path.to_string(),
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            Severity::Error => write!(f, "{}: {}", self.path, self.message),
            Severity::Warning => write!(f, "{}: warning: {}", self.path, self.message),
        }
    }
}

/// A file's display path (relative to the repo root) and its text.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub text: String,
}

/// Everything that parsed (even when other files did not).
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub taxonomy: Taxonomy,
    pub tools: Vec<Tool>,
    /// Display path per tool id.
    pub tool_paths: BTreeMap<String, String>,
}

fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".toml").unwrap_or(name)
}

pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_http_url(s: &str) -> bool {
    (s.starts_with("https://") || s.starts_with("http://"))
        && s.split_once("://")
            .is_some_and(|(_, rest)| !rest.is_empty() && !rest.contains(char::is_whitespace))
}

fn is_repo(s: &str) -> bool {
    let mut it = s.split('/');
    let ok = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    matches!((it.next(), it.next(), it.next()), (Some(a), Some(b), None) if ok(a) && ok(b))
}

fn normalize_url(u: &str) -> String {
    let u = u.trim().to_lowercase();
    u.trim_end_matches('/').to_string()
}

fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .map(|c| (strsim::levenshtein(name, c), c))
        .filter(|(d, c)| *d <= 2.max(c.len() / 3).min(4) && *d < name.len().max(1))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn check_lang(diags: &mut Vec<Diagnostic>, path: &str, what: &str, v: &RawLang) {
    for lang in v.missing() {
        diags.push(Diagnostic::error(
            path,
            format!("{what}.{lang} is missing or empty"),
        ));
    }
}

fn validate_taxonomy(path: &str, tax: &Taxonomy, diags: &mut Vec<Diagnostic>) {
    let mut facet_ids = BTreeSet::new();
    let mut names: BTreeMap<&str, String> = BTreeMap::new();
    for f in &tax.facets {
        if !facet_ids.insert(f.id.as_str()) {
            diags.push(Diagnostic::error(
                path,
                format!("duplicate facet \"{}\"", f.id),
            ));
        }
        if !is_slug(&f.id) {
            diags.push(Diagnostic::error(
                path,
                format!(
                    "facet id \"{}\" must be lowercase letters, digits and '-'",
                    f.id
                ),
            ));
        }
        check_lang(diags, path, &format!("facet \"{}\" label", f.id), &f.label);
        if let Some(max) = f.max
            && max < f.min
        {
            diags.push(Diagnostic::error(
                path,
                format!(
                    "facet \"{}\": max ({max}) is smaller than min ({})",
                    f.id, f.min
                ),
            ));
        }
        for t in &f.tags {
            if !is_slug(&t.id) {
                diags.push(Diagnostic::error(
                    path,
                    format!(
                        "tag id \"{}\" must be lowercase letters, digits and '-'",
                        t.id
                    ),
                ));
            }
            check_lang(diags, path, &format!("tag \"{}\" label", t.id), &t.label);
            check_lang(
                diags,
                path,
                &format!("tag \"{}\" description", t.id),
                &t.description,
            );
            if let Some(icon) = &t.icon
                && !crate::icons::has(icon)
            {
                diags.push(Diagnostic::error(
                    path,
                    format!(
                        "tag \"{}\": icon \"{icon}\" is not bundled in static/icons",
                        t.id
                    ),
                ));
            }
            let owner = format!("tag \"{}\"", t.id);
            if let Some(prev) = names.insert(&t.id, owner.clone()) {
                diags.push(Diagnostic::error(
                    path,
                    format!("\"{}\" is defined more than once ({prev})", t.id),
                ));
            }
            for a in &t.aliases {
                if let Some(prev) = names.insert(a, format!("alias of \"{}\"", t.id)) {
                    diags.push(Diagnostic::error(
                        path,
                        format!("alias \"{a}\" of \"{}\" collides with {prev}", t.id),
                    ));
                }
            }
        }
    }
}

fn facet_label(f: &Facet) -> &str {
    &f.id
}

fn validate_tool(path: &str, tool: &Tool, tax: &Taxonomy, diags: &mut Vec<Diagnostic>) {
    let f = &tool.file;
    if !is_slug(&tool.id) {
        diags.push(Diagnostic::error(
            path,
            format!(
                "file name \"{}\" must be lowercase letters, digits and '-'",
                tool.id
            ),
        ));
    }
    if let Some(id) = &f.id
        && id != &tool.id
    {
        diags.push(Diagnostic::error(
            path,
            format!("id \"{id}\" does not match the file name \"{}\"", tool.id),
        ));
    }
    if f.name.trim().is_empty() {
        diags.push(Diagnostic::error(path, "name is empty"));
    }
    if !is_http_url(&f.url) {
        diags.push(Diagnostic::error(
            path,
            format!("url \"{}\" is not an http(s) URL", f.url),
        ));
    }
    if let Some(repo) = &f.repo
        && !is_repo(repo)
    {
        diags.push(Diagnostic::error(
            path,
            format!("repo \"{repo}\" must look like \"owner/name\""),
        ));
    }
    if let Some(fav) = &f.favicon
        && !is_http_url(fav)
    {
        diags.push(Diagnostic::error(
            path,
            format!("favicon \"{fav}\" is not an http(s) URL"),
        ));
    }
    check_lang(diags, path, "summary", &f.summary);
    if f.platforms.is_empty() {
        diags.push(Diagnostic::error(path, "platforms must not be empty"));
    }
    if f.added.date.is_none() || f.added.time.is_some() {
        diags.push(Diagnostic::error(path, "added must be a date (YYYY-MM-DD)"));
    }
    for (k, v) in &f.links {
        if !is_http_url(v) {
            diags.push(Diagnostic::error(
                path,
                format!("links.{k} \"{v}\" is not an http(s) URL"),
            ));
        }
    }
    match f.status.as_deref() {
        None | Some("hidden") => {}
        Some(other) => diags.push(Diagnostic::error(
            path,
            format!("status \"{other}\" is not allowed (only \"hidden\")"),
        )),
    }
    if f.is_hidden() {
        if f.hidden_reason.is_none() {
            diags.push(Diagnostic::error(
                path,
                "status = \"hidden\" needs hidden_reason",
            ));
        }
        if f.hidden_since.is_none() {
            diags.push(Diagnostic::error(
                path,
                "status = \"hidden\" needs hidden_since",
            ));
        }
    }

    // Tags: known, not an alias, no duplicates.
    let mut seen = BTreeSet::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for tag in &f.tags {
        if !seen.insert(tag.as_str()) {
            diags.push(Diagnostic::error(path, format!("duplicate tag \"{tag}\"")));
            continue;
        }
        if let Some((facet, _)) = tax.find_tag(tag) {
            *counts.entry(facet.id.as_str()).or_default() += 1;
        } else if let Some(def) = tax.find_alias(tag) {
            diags.push(Diagnostic::error(
                path,
                format!("tag \"{tag}\" is an alias; use \"{}\"", def.id),
            ));
        } else {
            let all = tax
                .facets
                .iter()
                .flat_map(|f| f.tags.iter().map(|t| t.id.as_str()));
            let hint = closest(tag, all)
                .map(|c| format!(" (did you mean \"{c}\"?)"))
                .unwrap_or_default();
            diags.push(Diagnostic::error(
                path,
                format!("unknown tag \"{tag}\"{hint}"),
            ));
        }
    }
    for facet in &tax.facets {
        let n = counts.get(facet.id.as_str()).copied().unwrap_or(0);
        if n < facet.min {
            diags.push(Diagnostic::error(
                path,
                format!(
                    "facet \"{}\" needs at least {} tag(s), found {n}",
                    facet_label(facet),
                    facet.min
                ),
            ));
        }
        if let Some(max) = facet.max
            && n > max
        {
            diags.push(Diagnostic::error(
                path,
                format!(
                    "facet \"{}\" allows at most {max} tag(s), found {n}",
                    facet_label(facet)
                ),
            ));
        }
    }
}

/// Validate the whole data set. Errors make the command fail; warnings do not.
pub fn validate(
    tool_files: &[SourceFile],
    tags_file: &SourceFile,
    i18n_en: &SourceFile,
    i18n_ja: &SourceFile,
) -> (Vec<Diagnostic>, Project) {
    let mut diags = Vec::new();
    let mut project = Project::default();

    match Taxonomy::parse(&tags_file.text) {
        Ok(t) => {
            validate_taxonomy(&tags_file.path, &t, &mut diags);
            project.taxonomy = t;
        }
        Err(e) => diags.push(Diagnostic::error(&tags_file.path, e.message().to_string())),
    }

    for src in tool_files {
        let id = file_stem(&src.path);
        match Tool::parse(id, &src.text) {
            Ok(tool) => {
                validate_tool(&src.path, &tool, &project.taxonomy, &mut diags);
                project.tool_paths.insert(tool.id.clone(), src.path.clone());
                project.tools.push(tool);
            }
            Err(e) => diags.push(Diagnostic::error(&src.path, e.message().to_string())),
        }
    }

    // Duplicates across tools.
    let mut urls: BTreeMap<String, &str> = BTreeMap::new();
    let mut repos: BTreeMap<String, &str> = BTreeMap::new();
    for t in &project.tools {
        let path = project.tool_paths[&t.id].as_str();
        if let Some(prev) = urls.insert(normalize_url(&t.file.url), &t.id) {
            diags.push(Diagnostic::error(
                path,
                format!("url \"{}\" is already used by \"{prev}\"", t.file.url),
            ));
        }
        if let Some(repo) = &t.file.repo
            && let Some(prev) = repos.insert(repo.to_lowercase(), &t.id)
        {
            diags.push(Diagnostic::error(
                path,
                format!("repo \"{repo}\" is already used by \"{prev}\""),
            ));
        }
        if let Some(d) = &t.file.hidden_since
            && d.date.is_none()
        {
            diags.push(Diagnostic::error(
                path,
                format!("hidden_since \"{}\" must be a date", date_string(d)),
            ));
        }
    }

    // Unused tags (warning).
    let used: BTreeSet<&str> = project
        .tools
        .iter()
        .flat_map(|t| t.file.tags.iter().map(String::as_str))
        .collect();
    for f in &project.taxonomy.facets {
        for t in &f.tags {
            if !used.contains(t.id.as_str()) {
                diags.push(Diagnostic::warning(
                    &tags_file.path,
                    format!("tag \"{}\" is not used by any tool", t.id),
                ));
            }
        }
    }

    // i18n key parity.
    match (i18n_keys(&i18n_en.text), i18n_keys(&i18n_ja.text)) {
        (Ok(en), Ok(ja)) => {
            for k in en.keys().filter(|k| !ja.contains_key(*k)) {
                diags.push(Diagnostic::error(
                    &i18n_ja.path,
                    format!("key \"{k}\" is missing (present in {})", i18n_en.path),
                ));
            }
            for k in ja.keys().filter(|k| !en.contains_key(*k)) {
                diags.push(Diagnostic::error(
                    &i18n_en.path,
                    format!("key \"{k}\" is missing (present in {})", i18n_ja.path),
                ));
            }
        }
        (en, ja) => {
            if let Err(e) = en {
                diags.push(Diagnostic::error(&i18n_en.path, e.message().to_string()));
            }
            if let Err(e) = ja {
                diags.push(Diagnostic::error(&i18n_ja.path, e.message().to_string()));
            }
        }
    }

    (diags, project)
}

/// Check that the category dropdown of the "suggest a tool" issue form
/// offers exactly the category labels of `tags.toml` (plus "Not sure").
pub fn check_issue_form(form: &SourceFile, tax: &Taxonomy) -> Vec<Diagnostic> {
    use crate::guide::{FORM_CATEGORY_ID, expected_category_options, form_dropdown_options};
    let expected = expected_category_options(tax);
    match form_dropdown_options(&form.text, FORM_CATEGORY_ID) {
        Some(found) if found == expected => Vec::new(),
        found => {
            let list = expected
                .iter()
                .map(|o| format!("        - {o}"))
                .collect::<Vec<_>>()
                .join("\n");
            let what = match found {
                Some(_) => "options differ from the categories in data/tags.toml",
                None => "has no dropdown with id \"category\"",
            };
            vec![Diagnostic::error(
                &form.path,
                format!("category dropdown {what}; expected options:\n{list}"),
            )]
        }
    }
}

pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAGS: &str = r#"
[[facet]]
id = "category"
label = { en = "Category", ja = "カテゴリ" }
min = 1
max = 3
  [[facet.tag]]
  id = "coding-agent"
  label = { en = "Coding agent", ja = "コーディングエージェント" }
  description = { en = "d", ja = "d" }
  aliases = ["agent"]
  [[facet.tag]]
  id = "sandbox"
  label = { en = "Sandbox", ja = "サンドボックス" }
  description = { en = "d", ja = "d" }
  [[facet.tag]]
  id = "memory"
  label = { en = "Memory", ja = "メモリ" }
  description = { en = "d", ja = "d" }
  [[facet.tag]]
  id = "gateway"
  label = { en = "Gateway", ja = "ゲートウェイ" }
  description = { en = "d", ja = "d" }

[[facet]]
id = "interface"
label = { en = "Interface", ja = "インターフェース" }
min = 1
  [[facet.tag]]
  id = "cli"
  label = { en = "CLI", ja = "CLI" }
  description = { en = "d", ja = "d" }
"#;

    fn tool(extra_tags: &str, summary: &str) -> String {
        format!(
            r#"name = "X"
url = "https://x.dev"
{summary}
pricing = "free"
open_source = true
platforms = ["linux"]
tags = [{extra_tags}]
added = 2026-11-02
"#
        )
    }

    const SUMMARY: &str = "summary.en = \"e\"\nsummary.ja = \"j\"";

    fn run(tools: &[(&str, String)], tags: &str) -> Vec<String> {
        let files: Vec<SourceFile> = tools
            .iter()
            .map(|(n, t)| SourceFile {
                path: format!("data/tools/{n}.toml"),
                text: t.clone(),
            })
            .collect();
        let i18n = |p: &str, t: &str| SourceFile {
            path: p.into(),
            text: t.into(),
        };
        let (d, _) = validate(
            &files,
            &i18n("data/tags.toml", tags),
            &i18n("i18n/en.toml", "a = \"1\"\n"),
            &i18n("i18n/ja.toml", "a = \"1\"\n"),
        );
        d.iter().map(|d| d.to_string()).collect()
    }

    fn has(out: &[String], needle: &str) -> bool {
        out.iter().any(|l| l.contains(needle))
    }

    #[test]
    fn valid_data_has_no_errors() {
        let out = run(&[("x", tool("\"coding-agent\", \"cli\"", SUMMARY))], TAGS);
        assert!(!out.iter().any(|l| !l.contains("warning")), "{out:?}");
    }

    #[test]
    fn unknown_tag_suggests_closest() {
        let out = run(&[("x", tool("\"coding-agent\", \"clii\"", SUMMARY))], TAGS);
        assert!(
            has(
                &out,
                "data/tools/x.toml: unknown tag \"clii\" (did you mean \"cli\"?)"
            ),
            "{out:?}"
        );
    }

    #[test]
    fn alias_points_to_canonical() {
        let out = run(&[("x", tool("\"agent\", \"cli\"", SUMMARY))], TAGS);
        assert!(
            has(&out, "tag \"agent\" is an alias; use \"coding-agent\""),
            "{out:?}"
        );
    }

    #[test]
    fn category_count_zero_and_four_fail() {
        let zero = run(&[("x", tool("\"cli\"", SUMMARY))], TAGS);
        assert!(
            has(&zero, "facet \"category\" needs at least 1 tag(s), found 0"),
            "{zero:?}"
        );
        let four = run(
            &[(
                "x",
                tool(
                    "\"coding-agent\", \"sandbox\", \"memory\", \"gateway\", \"cli\"",
                    SUMMARY,
                ),
            )],
            TAGS,
        );
        assert!(
            has(&four, "facet \"category\" allows at most 3 tag(s), found 4"),
            "{four:?}"
        );
        let two = run(
            &[("x", tool("\"coding-agent\", \"sandbox\", \"cli\"", SUMMARY))],
            TAGS,
        );
        assert!(!two.iter().any(|l| !l.contains("warning")), "{two:?}");
    }

    #[test]
    fn missing_summary_ja_fails() {
        let out = run(
            &[("x", tool("\"coding-agent\", \"cli\"", "summary.en = \"e\""))],
            TAGS,
        );
        assert!(has(&out, "summary.ja is missing"), "{out:?}");
    }

    #[test]
    fn missing_required_field_fails() {
        let out = run(&[("x", "name = \"X\"\n".to_string())], TAGS);
        assert!(has(&out, "data/tools/x.toml: missing field"), "{out:?}");
    }

    #[test]
    fn duplicate_url_and_repo() {
        let a = tool("\"coding-agent\", \"cli\"", SUMMARY) + "repo = \"o/n\"\n";
        let out = run(&[("a", a.clone()), ("b", a)], TAGS);
        assert!(
            has(&out, "url \"https://x.dev\" is already used by \"a\""),
            "{out:?}"
        );
        assert!(
            has(&out, "repo \"o/n\" is already used by \"a\""),
            "{out:?}"
        );
    }

    #[test]
    fn favicon_must_be_http_url() {
        let ok = tool("\"coding-agent\", \"cli\"", SUMMARY) + "favicon = \"https://x.dev/i.png\"\n";
        assert!(!has(&run(&[("a", ok)], TAGS), "favicon"));
        let bad = tool("\"coding-agent\", \"cli\"", SUMMARY) + "favicon = \"icon.png\"\n";
        assert!(has(
            &run(&[("a", bad)], TAGS),
            "favicon \"icon.png\" is not an http(s) URL"
        ),);
    }

    #[test]
    fn id_must_match_file_name() {
        let t = format!(
            "id = \"other\"\n{}",
            tool("\"coding-agent\", \"cli\"", SUMMARY)
        );
        let out = run(&[("x", t)], TAGS);
        assert!(
            has(&out, "id \"other\" does not match the file name \"x\""),
            "{out:?}"
        );
    }

    #[test]
    fn tag_labels_need_both_languages() {
        let tags = TAGS.replace(
            "label = { en = \"Sandbox\", ja = \"サンドボックス\" }",
            "label = { en = \"Sandbox\" }",
        );
        let out = run(&[("x", tool("\"coding-agent\", \"cli\"", SUMMARY))], &tags);
        assert!(has(&out, "tag \"sandbox\" label.ja is missing"), "{out:?}");
    }

    #[test]
    fn unknown_tag_icon_fails_and_bundled_passes() {
        let ok = TAGS.replace("id = \"sandbox\"", "id = \"sandbox\"\n  icon = \"box\"");
        let out = run(&[("x", tool("\"coding-agent\", \"cli\"", SUMMARY))], &ok);
        assert!(!has(&out, "icon"), "{out:?}");
        let bad = TAGS.replace(
            "id = \"sandbox\"",
            "id = \"sandbox\"\n  icon = \"no-such-icon\"",
        );
        let out = run(&[("x", tool("\"coding-agent\", \"cli\"", SUMMARY))], &bad);
        assert!(
            has(
                &out,
                "tag \"sandbox\": icon \"no-such-icon\" is not bundled"
            ),
            "{out:?}"
        );
    }

    #[test]
    fn unused_tag_is_only_a_warning() {
        let out = run(&[("x", tool("\"coding-agent\", \"cli\"", SUMMARY))], TAGS);
        assert!(
            has(
                &out,
                "tags.toml: warning: tag \"memory\" is not used by any tool"
            ),
            "{out:?}"
        );
    }

    #[test]
    fn i18n_keys_must_match() {
        let f = |p: &str, t: &str| SourceFile {
            path: p.into(),
            text: t.into(),
        };
        let (d, _) = validate(
            &[],
            &f("data/tags.toml", ""),
            &f("i18n/en.toml", "a = \"1\"\nb = \"2\"\n"),
            &f("i18n/ja.toml", "a = \"1\"\nc = \"3\"\n"),
        );
        let out: Vec<String> = d.iter().map(|d| d.to_string()).collect();
        assert!(has(&out, "i18n/ja.toml: key \"b\" is missing"), "{out:?}");
        assert!(has(&out, "i18n/en.toml: key \"c\" is missing"), "{out:?}");
    }

    #[test]
    fn hidden_needs_reason_and_since() {
        let t = tool("\"coding-agent\", \"cli\"", SUMMARY) + "status = \"hidden\"\n";
        let out = run(&[("x", t)], TAGS);
        assert!(has(&out, "needs hidden_reason"), "{out:?}");
        assert!(has(&out, "needs hidden_since"), "{out:?}");
    }

    #[test]
    fn facet_min_greater_than_max() {
        let tags = TAGS.replace("min = 1\nmax = 3", "min = 4\nmax = 3");
        let out = run(&[], &tags);
        assert!(has(&out, "max (3) is smaller than min (4)"), "{out:?}");
    }

    #[test]
    fn issue_form_categories_must_match_tags() {
        let tax = Taxonomy::parse(TAGS).unwrap();
        let form = |opts: &str| SourceFile {
            path: "form.yml".into(),
            text: format!(
                "body:\n  - type: dropdown\n    id: category\n    attributes:\n      options:\n{opts}"
            ),
        };
        let good = form(
            "        - Coding agent\n        - Sandbox\n        - Memory\n        - Gateway\n        - Not sure / わからない\n",
        );
        assert!(check_issue_form(&good, &tax).is_empty());
        let bad = form("        - Coding agent\n        - Not sure / わからない\n");
        let d = check_issue_form(&bad, &tax);
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("- Sandbox"), "{}", d[0].message);
        assert_eq!(d[0].path, "form.yml");
        let none = SourceFile {
            path: "form.yml".into(),
            text: "body: []\n".into(),
        };
        assert_eq!(check_issue_form(&none, &tax).len(), 1);
    }
}
