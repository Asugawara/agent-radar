use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Facet that is shown as badges in the "Category" column.
pub const CATEGORY_FACET: &str = "category";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pricing {
    Free,
    Freemium,
    Paid,
}

impl Pricing {
    pub fn as_str(self) -> &'static str {
        match self {
            Pricing::Free => "free",
            Pricing::Freemium => "freemium",
            Pricing::Paid => "paid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
    Web,
    Ios,
    Android,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Web => "web",
            Platform::Ios => "ios",
            Platform::Android => "android",
        }
    }
}

/// A text in both languages (all fields required).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lang {
    pub en: String,
    pub ja: String,
}

impl Lang {
    pub fn get(&self, lang: &str) -> &str {
        if lang == "ja" { &self.ja } else { &self.en }
    }
}

/// A text as written in a TOML file; either language may be missing
/// (validation reports that).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RawLang {
    pub en: Option<String>,
    pub ja: Option<String>,
}

impl RawLang {
    /// Name of the first missing/empty language, if any.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.en.as_deref().is_none_or(|s| s.trim().is_empty()) {
            out.push("en");
        }
        if self.ja.as_deref().is_none_or(|s| s.trim().is_empty()) {
            out.push("ja");
        }
        out
    }

    pub fn to_lang(&self) -> Lang {
        Lang {
            en: self.en.clone().unwrap_or_default(),
            ja: self.ja.clone().unwrap_or_default(),
        }
    }
}

/// Contents of `data/tools/<id>.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolFile {
    /// Optional; when present it must equal the file name.
    pub id: Option<String>,
    pub name: String,
    pub url: String,
    pub repo: Option<String>,
    /// Optional icon URL; overrides what `radar favicons` would find.
    pub favicon: Option<String>,
    pub summary: RawLang,
    pub pricing: Pricing,
    pub open_source: bool,
    pub platforms: Vec<Platform>,
    pub tags: Vec<String>,
    pub added: toml::value::Datetime,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    pub status: Option<String>,
    pub hidden_reason: Option<String>,
    pub hidden_since: Option<toml::value::Datetime>,
}

impl ToolFile {
    pub fn is_hidden(&self) -> bool {
        self.status.as_deref() == Some("hidden")
    }
}

/// A tool together with the id taken from its file name.
#[derive(Debug, Clone)]
pub struct Tool {
    pub id: String,
    pub file: ToolFile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagDef {
    pub id: String,
    #[serde(default)]
    pub label: RawLang,
    #[serde(default)]
    pub description: RawLang,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Lucide icon name (must be bundled, see `icons`); category tags only in practice.
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facet {
    pub id: String,
    #[serde(default)]
    pub label: RawLang,
    #[serde(default)]
    pub min: usize,
    pub max: Option<usize>,
    #[serde(default, rename = "tag")]
    pub tags: Vec<TagDef>,
}

/// Contents of `data/tags.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Taxonomy {
    #[serde(default, rename = "facet")]
    pub facets: Vec<Facet>,
}

impl Taxonomy {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Facet index and tag for a canonical tag id.
    pub fn find_tag(&self, id: &str) -> Option<(&Facet, &TagDef)> {
        self.facets
            .iter()
            .find_map(|f| f.tags.iter().find(|t| t.id == id).map(|t| (f, t)))
    }

    /// The canonical tag id that `alias` stands for.
    pub fn find_alias(&self, alias: &str) -> Option<&TagDef> {
        self.facets
            .iter()
            .flat_map(|f| f.tags.iter())
            .find(|t| t.aliases.iter().any(|a| a == alias))
    }
}

impl Tool {
    pub fn parse(id: &str, text: &str) -> Result<Self, toml::de::Error> {
        let file: ToolFile = toml::from_str(text)?;
        Ok(Tool {
            id: id.to_string(),
            file,
        })
    }
}

/// Contents of `site.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct SiteConfig {
    pub title: String,
    /// Scheme and host, e.g. `https://asugawara.github.io` (for absolute URLs).
    pub origin: String,
    pub base_url: String,
    pub repo: String,
}

impl SiteConfig {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

/// Flattened keys of an `i18n/*.toml` file (`a.b.c` for nested tables).
pub fn i18n_keys(text: &str) -> Result<BTreeMap<String, String>, toml::de::Error> {
    fn walk(prefix: &str, v: &toml::Value, out: &mut BTreeMap<String, String>) {
        match v {
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&key, v, out);
                }
            }
            toml::Value::String(s) => {
                out.insert(prefix.to_string(), s.clone());
            }
            other => {
                out.insert(prefix.to_string(), other.to_string());
            }
        }
    }
    let value: toml::Value = toml::from_str(text)?;
    let mut out = BTreeMap::new();
    walk("", &value, &mut out);
    Ok(out)
}
