//! Reading, writing and canonicalizing `data/tools/<id>.toml` with
//! `toml_edit`, so comments survive. The link checker reuses these helpers
//! (remove a `[links]` key, set `status = "hidden"`).

use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Value};

/// Canonical order of the root keys. `[links]` is a table and must come last.
pub const KEY_ORDER: &[&str] = &[
    "id",
    "name",
    "url",
    "repo",
    "favicon",
    "summary",
    "pricing",
    "open_source",
    "platforms",
    "tags",
    "added",
    "status",
    "hidden_reason",
    "hidden_since",
    "links",
];

pub fn parse_doc(text: &str) -> Result<DocumentMut> {
    text.parse::<DocumentMut>()
        .map_err(|e| anyhow::anyhow!("{}", e.message()))
}

pub fn read_doc(path: &Path) -> Result<DocumentMut> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse_doc(&text).with_context(|| format!("parse {}", path.display()))
}

/// Writes the document in canonical form.
pub fn write_doc(path: &Path, doc: &mut DocumentMut) -> Result<()> {
    canonicalize(doc);
    std::fs::write(path, doc.to_string()).with_context(|| format!("write {}", path.display()))
}

/// Remove `links.<key>`; drops the `[links]` table when it becomes empty.
/// Returns whether something was removed.
pub fn remove_link(doc: &mut DocumentMut, key: &str) -> bool {
    let Some(links) = doc.get_mut("links").and_then(Item::as_table_like_mut) else {
        return false;
    };
    let removed = links.remove(key).is_some();
    if removed && links.is_empty() {
        doc.remove("links");
    }
    removed
}

/// Hide a tool: `status`, `hidden_reason`, `hidden_since` (`YYYY-MM-DD`).
pub fn set_hidden(doc: &mut DocumentMut, reason: &str, since: &str) {
    doc["status"] = toml_edit::value("hidden");
    doc["hidden_reason"] = toml_edit::value(reason);
    if let Ok(d) = since.parse::<toml_edit::Datetime>() {
        doc["hidden_since"] = toml_edit::value(d);
    }
}

/// Undo [`set_hidden`].
pub fn clear_hidden(doc: &mut DocumentMut) {
    for k in ["status", "hidden_reason", "hidden_since"] {
        doc.remove(k);
    }
}

/// Reorder keys into [`KEY_ORDER`] (unknown keys follow, alphabetically),
/// write `summary` as dotted keys, sort `links`, and tidy one-line arrays.
/// Comments attached to keys move with them.
pub fn canonicalize(doc: &mut DocumentMut) {
    let root = doc.as_table_mut();
    let mut old = std::mem::take(root);
    let mut keys: Vec<String> = old.iter().map(|(k, _)| k.to_string()).collect();
    keys.sort_by_key(|k| {
        (
            KEY_ORDER
                .iter()
                .position(|o| o == k)
                .unwrap_or(KEY_ORDER.len() - 1),
            k.clone(),
        )
    });
    // Unknown keys must not land after `links` (a table).
    let (known, unknown): (Vec<_>, Vec<_>) = keys
        .into_iter()
        .partition(|k| KEY_ORDER.contains(&k.as_str()));
    let order: Vec<String> = known
        .iter()
        .filter(|k| *k != "links")
        .chain(unknown.iter())
        .chain(known.iter().filter(|k| *k == "links"))
        .cloned()
        .collect();
    for k in order {
        if let Some((key, mut item)) = old.remove_entry(&k) {
            tidy(&k, &mut item);
            root.insert_formatted(&key, item);
        }
    }
}

fn tidy(key: &str, item: &mut Item) {
    match key {
        "summary" => {
            if let Some(t) = item.as_table_like_mut() {
                t.sort_values();
            }
            // Convert an inline table to dotted keys.
            if let Item::Value(Value::InlineTable(it)) = item {
                it.set_dotted(true);
                it.sort_values();
            } else if let Item::Table(t) = item {
                t.set_dotted(true);
            }
        }
        "links" => {
            if let Item::Table(t) = item {
                t.set_implicit(false);
                t.sort_values();
            }
        }
        "platforms" | "tags" => {
            if let Item::Value(Value::Array(a)) = item
                && !a.to_string().contains('#')
            {
                let (prefix, suffix) = (a.decor().prefix().cloned(), a.decor().suffix().cloned());
                a.fmt();
                if let Some(p) = prefix {
                    a.decor_mut().set_prefix(p);
                }
                if let Some(s) = suffix {
                    a.decor_mut().set_suffix(s);
                }
            }
        }
        _ => {}
    }
}

/// Canonical text of `text`.
pub fn canonical_text(text: &str) -> Result<String> {
    let mut doc = parse_doc(text)?;
    canonicalize(&mut doc);
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_is_stable_and_keeps_comments() {
        let src = r#"# my note
tags = ["b", "a"]
name = "X"
summary.ja = "j"
summary.en = "e"
url = "https://x.dev" # trailing
pricing = "free"
open_source = true
platforms = ["linux"]
added = 2026-11-02

[links]
z = "https://z.dev"
# about a
a = "https://a.dev"
"#;
        let once = canonical_text(src).unwrap();
        let twice = canonical_text(&once).unwrap();
        assert_eq!(once, twice, "not idempotent:\n{once}");
        assert!(once.contains("# my note"));
        assert!(once.contains("# trailing"));
        assert!(once.contains("# about a"));
        let name = once.find("name =").unwrap();
        let url = once.find("url =").unwrap();
        let summary_en = once.find("summary.en").unwrap();
        let summary_ja = once.find("summary.ja").unwrap();
        let links = once.find("[links]").unwrap();
        assert!(name < url && url < summary_en && summary_en < summary_ja && summary_ja < links);
        assert!(once.find("a = \"https://a.dev\"").unwrap() < once.find("z =").unwrap());
    }

    #[test]
    fn hide_and_remove_link() {
        let src = "name = \"X\"\n\n[links]\ndocs = \"https://d.dev\"\n";
        let mut doc = parse_doc(src).unwrap();
        assert!(remove_link(&mut doc, "docs"));
        set_hidden(&mut doc, "url: 404", "2026-11-05");
        canonicalize(&mut doc);
        let out = doc.to_string();
        assert!(!out.contains("links"));
        assert!(out.contains("status = \"hidden\""));
        assert!(out.contains("hidden_since = 2026-11-05"));
        clear_hidden(&mut doc);
        assert!(!doc.to_string().contains("status"));
    }
}
