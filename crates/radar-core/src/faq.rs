//! The FAQ above the table (`data/faq.toml`): validation of the file and the
//! answers, built from the data (the most-starred tools for a query, counts,
//! the build date).

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::icons::short_count;
use crate::model::{Lang, Taxonomy};
use crate::query::{ALL_PLATFORMS, Query, decode};
use crate::site::SiteData;
use crate::validate::{Diagnostic, Severity, SourceFile, is_slug};

pub const FAQ_PATH: &str = "data/faq.toml";

/// Placeholders an answer may use.
const KNOWN: [&str; 4] = ["n", "top", "date", "total"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaqFile {
    #[serde(default)]
    pub faq: Vec<FaqEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaqEntry {
    pub id: String,
    pub question: Lang,
    pub answer: Lang,
    /// The table's address keys (`tag=…&sort=-stars`), without `?`.
    #[serde(default)]
    pub query: Option<String>,
    /// How many tools `{top}` names.
    #[serde(default = "default_top")]
    pub top: usize,
}

fn default_top() -> usize {
    3
}

fn push(diags: &mut Vec<Diagnostic>, path: &str, message: impl Into<String>) {
    diags.push(Diagnostic {
        path: path.to_string(),
        severity: Severity::Error,
        message: message.into(),
    });
}

/// The `{word}` placeholders in `s`.
pub fn placeholders(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        let name = &after[..close];
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            out.insert(name.to_string());
        }
        rest = &after[close + 1..];
    }
    out
}

/// Problems with the `query` of an entry (the keys [`Query::parse`] reads
/// and their values, which it would otherwise ignore silently).
fn query_problems(query: &str, tax: &Taxonomy) -> Vec<String> {
    let mut out = Vec::new();
    if query.trim().is_empty() {
        out.push("query is empty".to_string());
    }
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = decode(v);
        match k {
            "tag" => {
                let id = v.strip_prefix('-').unwrap_or(v.as_str());
                if tax.find_tag(id).is_none() {
                    out.push(format!("query: unknown tag \"{id}\""));
                }
            }
            "sort" => {
                let name = v.strip_prefix('-').unwrap_or(v.as_str());
                if !["name", "stars", "updated", "added"].contains(&name) {
                    out.push(format!("query: unknown sort \"{name}\""));
                }
            }
            "pricing" => {
                if !["free", "freemium", "paid"].contains(&v.as_str()) {
                    out.push(format!("query: unknown pricing \"{v}\""));
                }
            }
            "platform" => {
                if !ALL_PLATFORMS.iter().any(|p| p.as_str() == v) {
                    out.push(format!("query: unknown platform \"{v}\""));
                }
            }
            other => out.push(format!(
                "query: key \"{other}\" is not one of tag, sort, pricing, platform"
            )),
        }
    }
    out
}

/// Check `data/faq.toml` against the taxonomy. Errors name the file and the entry id.
pub fn check_faq(file: &SourceFile, tax: &Taxonomy) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let path = file.path.as_str();
    let parsed: FaqFile = match toml::from_str(&file.text) {
        Ok(f) => f,
        Err(e) => {
            push(&mut diags, path, format!("cannot parse: {}", e.message()));
            return diags;
        }
    };
    let mut ids = BTreeSet::new();
    for entry in &parsed.faq {
        let id = entry.id.as_str();
        let at = |m: &str| format!("faq \"{id}\": {m}");
        if !ids.insert(id) {
            push(&mut diags, path, at("duplicate id"));
        }
        if !is_slug(id) {
            push(
                &mut diags,
                path,
                at("id must be lowercase letters, digits and '-'"),
            );
        }
        let texts = [
            ("question.en", &entry.question.en),
            ("question.ja", &entry.question.ja),
            ("answer.en", &entry.answer.en),
            ("answer.ja", &entry.answer.ja),
        ];
        for (key, text) in texts {
            if text.trim().is_empty() {
                push(&mut diags, path, at(&format!("{key} is empty")));
            }
        }
        if entry.top == 0 {
            push(&mut diags, path, at("top must be at least 1"));
        }
        let ph_en = placeholders(&entry.answer.en);
        let ph_ja = placeholders(&entry.answer.ja);
        for (key, ph) in [("answer.en", &ph_en), ("answer.ja", &ph_ja)] {
            for p in ph {
                if !KNOWN.contains(&p.as_str()) {
                    push(
                        &mut diags,
                        path,
                        at(&format!("{key} has unknown placeholder {{{p}}}")),
                    );
                }
            }
        }
        if ph_en != ph_ja {
            push(
                &mut diags,
                path,
                at("answer.en and answer.ja use different placeholders"),
            );
        }
        match &entry.query {
            None => {
                if ph_en.union(&ph_ja).any(|p| p == "n" || p == "top") {
                    push(&mut diags, path, at("{n} and {top} need a query"));
                }
            }
            Some(q) => {
                for problem in query_problems(q, tax) {
                    push(&mut diags, path, at(&problem));
                }
            }
        }
    }
    diags
}

/// Names for an answer: `A (25k stars), B and C` (en) or `A（★25k）、B、C` (ja).
pub fn join_top(items: &[(String, Option<u64>)], lang: &str) -> String {
    if lang == "ja" {
        return items
            .iter()
            .map(|(name, stars)| match stars {
                Some(s) => format!("{name}（★{}）", short_count(*s)),
                None => name.clone(),
            })
            .collect::<Vec<_>>()
            .join("、");
    }
    let parts: Vec<String> = items
        .iter()
        .map(|(name, stars)| match stars {
            Some(s) => format!("{name} ({} stars)", short_count(*s)),
            None => name.clone(),
        })
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

pub struct RenderedFaq {
    pub id: String,
    pub question: String,
    pub answer: String,
    /// The query (without `?`) and the number of tools it matches, for "See all".
    pub link: Option<(String, usize)>,
}

/// The FAQ entry in `lang`, or `None` when its query matches no tool.
pub fn render(entry: &FaqEntry, data: &SiteData, lang: &str, date: &str) -> Option<RenderedFaq> {
    let mut link = None;
    let mut top_text = String::new();
    let mut n = 0;
    if let Some(query) = &entry.query {
        let tools = Query::parse(query, data).apply(data, lang, &BTreeSet::new());
        if tools.is_empty() {
            return None;
        }
        let items: Vec<(String, Option<u64>)> = tools
            .iter()
            .take(entry.top)
            .map(|t| (t.name.clone(), t.stars))
            .collect();
        top_text = join_top(&items, lang);
        n = tools.len();
        link = Some((query.clone(), n));
    }
    let answer = entry
        .answer
        .get(lang)
        .replace("{top}", &top_text)
        .replace("{n}", &n.to_string())
        .replace("{total}", &data.tools.len().to_string())
        .replace("{date}", date);
    Some(RenderedFaq {
        id: entry.id.clone(),
        question: entry.question.get(lang).to_string(),
        answer,
        link,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[(&str, Option<u64>)]) -> Vec<(String, Option<u64>)> {
        v.iter().map(|(n, s)| (n.to_string(), *s)).collect()
    }

    #[test]
    fn join_top_en() {
        assert_eq!(
            join_top(&items(&[("A", Some(25_000))]), "en"),
            "A (25k stars)"
        );
        assert_eq!(join_top(&items(&[("A", None)]), "en"), "A");
        assert_eq!(
            join_top(&items(&[("A", None), ("B", None)]), "en"),
            "A and B"
        );
        assert_eq!(
            join_top(
                &items(&[("A", Some(1_000)), ("B", None), ("C", Some(500))]),
                "en"
            ),
            "A (1k stars), B and C (500 stars)"
        );
        assert_eq!(join_top(&[], "en"), "");
    }

    #[test]
    fn join_top_ja() {
        assert_eq!(join_top(&items(&[("A", Some(25_000))]), "ja"), "A（★25k）");
        assert_eq!(join_top(&items(&[("A", None)]), "ja"), "A");
        assert_eq!(
            join_top(
                &items(&[("A", Some(1_000)), ("B", None), ("C", Some(500))]),
                "ja"
            ),
            "A（★1k）、B、C（★500）"
        );
    }

    #[test]
    fn placeholders_are_words_in_braces() {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(
            placeholders("{top} of {n} as of {date}, {n} again"),
            set(&["top", "n", "date"])
        );
        assert_eq!(placeholders("no braces"), set(&[]));
        assert_eq!(placeholders("{} and { x } and {foo"), set(&[]));
        assert_eq!(placeholders("{foo_1}"), set(&["foo_1"]));
    }
}
