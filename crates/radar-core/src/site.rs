use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Lang, Platform, Pricing, RawLang, Taxonomy, Tool};

/// Metadata fetched from GitHub (`radar enrich`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoMeta {
    pub stars: u64,
    pub pushed_at: String,
    pub archived: bool,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteTag {
    pub id: String,
    pub label: Lang,
    /// Short text for the table's tiles; equals `label` when the tag has no `short`.
    pub short: Lang,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteFacet {
    pub id: String,
    pub label: Lang,
    pub min: usize,
    pub max: Option<usize>,
    pub tags: Vec<SiteTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteTool {
    pub id: String,
    pub name: String,
    pub url: String,
    pub repo: Option<String>,
    pub summary: Lang,
    pub pricing: Pricing,
    pub open_source: bool,
    pub platforms: Vec<Platform>,
    pub tags: Vec<String>,
    /// `YYYY-MM-DD`
    pub added: String,
    pub links: BTreeMap<String, String>,
    pub stars: Option<u64>,
    pub pushed_at: Option<String>,
    pub archived: bool,
    /// Path of the favicon below the site root (`/favicons/x.png`), if one was fetched.
    #[serde(default)]
    pub favicon: Option<String>,
}

/// What is embedded in the page (`#site-data`) and written to `tools.json`.
/// Hidden tools are not part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteData {
    /// RFC 3339, UTC.
    pub generated_at: String,
    pub facets: Vec<SiteFacet>,
    pub tools: Vec<SiteTool>,
}

/// A tag's `short` text per language; a missing or empty language falls back to its `label`.
fn short_or_label(short: &RawLang, label: &Lang) -> Lang {
    let pick = |s: &Option<String>, fallback: &String| {
        s.as_deref()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| fallback.clone(), str::to_string)
    };
    Lang {
        en: pick(&short.en, &label.en),
        ja: pick(&short.ja, &label.ja),
    }
}

/// Case-insensitive lookup of `owner/name` in the enrich map.
fn lookup<'a>(enrich: &'a BTreeMap<String, RepoMeta>, repo: &str) -> Option<&'a RepoMeta> {
    enrich.get(repo).or_else(|| {
        enrich
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(repo))
            .map(|(_, v)| v)
    })
}

/// Build the data for the site from validated input. Visible tools are
/// sorted by name; stars etc. come from `enrich` (empty map: all `None`).
pub fn build_site_data(
    taxonomy: &Taxonomy,
    tools: &[Tool],
    enrich: &BTreeMap<String, RepoMeta>,
    generated_at: String,
) -> SiteData {
    let facets = taxonomy
        .facets
        .iter()
        .map(|f| SiteFacet {
            id: f.id.clone(),
            label: f.label.to_lang(),
            min: f.min,
            max: f.max,
            tags: f
                .tags
                .iter()
                .map(|t| {
                    let label = t.label.to_lang();
                    SiteTag {
                        id: t.id.clone(),
                        short: short_or_label(&t.short, &label),
                        label,
                        icon: t.icon.clone(),
                    }
                })
                .collect(),
        })
        .collect();

    let mut out: Vec<SiteTool> = tools
        .iter()
        .filter(|t| !t.file.is_hidden())
        .map(|t| {
            let f = &t.file;
            let meta = f.repo.as_deref().and_then(|r| lookup(enrich, r));
            SiteTool {
                id: t.id.clone(),
                name: f.name.clone(),
                url: f.url.clone(),
                repo: f.repo.clone(),
                summary: f.summary.to_lang(),
                pricing: f.pricing,
                open_source: f.open_source,
                platforms: f.platforms.clone(),
                tags: f.tags.clone(),
                added: date_string(&f.added),
                links: f.links.clone(),
                stars: meta.map(|m| m.stars),
                pushed_at: meta.map(|m| m.pushed_at.clone()),
                archived: meta.is_some_and(|m| m.archived),
                favicon: None,
            }
        })
        .collect();
    out.sort_by_key(|t| t.name.to_lowercase());

    SiteData {
        generated_at,
        facets,
        tools: out,
    }
}

/// `YYYY-MM-DD` of a TOML date (time part, if any, is dropped).
pub fn date_string(d: &toml::value::Datetime) -> String {
    match d.date {
        Some(date) => format!("{:04}-{:02}-{:02}", date.year, date.month, date.day),
        None => d.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_short_text_falls_back_to_label() {
        let tax = Taxonomy::parse(
            "[[facet]]\n\
             id = \"works-with\"\n\
             [[facet.tag]]\n\
             id = \"a\"\n\
             label = { en = \"Claude Code\", ja = \"Claude Code\" }\n\
             short = { en = \"Claude\", ja = \"Claude\" }\n\
             [[facet.tag]]\n\
             id = \"b\"\n\
             label = { en = \"Codex\", ja = \"Codex 日本\" }\n",
        )
        .unwrap();
        let data = build_site_data(&tax, &[], &BTreeMap::new(), "t".into());
        let tags = &data.facets[0].tags;
        assert_eq!(tags[0].short.en, "Claude");
        assert_eq!(tags[0].short.ja, "Claude");
        assert_eq!(tags[0].label.en, "Claude Code");
        assert_eq!(tags[1].short, tags[1].label);
    }
}
