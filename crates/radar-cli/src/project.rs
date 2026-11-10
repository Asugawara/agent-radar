//! Loading the repository's data files (the only place that reads them).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use radar_core::{Diagnostic, Project, SiteConfig, SourceFile, has_errors, validate};

pub struct Sources {
    pub root: PathBuf,
    pub tools: Vec<SourceFile>,
    pub tags: SourceFile,
    pub en: SourceFile,
    pub ja: SourceFile,
    /// The issue form for suggestions; absent in fixtures.
    pub issue_form: Option<SourceFile>,
    /// The FAQ above the table; absent in fixtures.
    pub faq: Option<SourceFile>,
}

fn read(root: &Path, rel: &str) -> Result<SourceFile> {
    let text =
        std::fs::read_to_string(root.join(rel)).with_context(|| format!("{rel}: cannot read"))?;
    Ok(SourceFile {
        path: rel.to_string(),
        text,
    })
}

impl Sources {
    pub fn load(root: &Path) -> Result<Self> {
        let dir = root.join("data/tools");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .with_context(|| format!("{}: cannot read", dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".toml"))
            .collect();
        names.sort();
        let tools = names
            .iter()
            .map(|n| read(root, &format!("data/tools/{n}")))
            .collect::<Result<Vec<_>>>()?;
        Ok(Sources {
            root: root.to_path_buf(),
            tools,
            tags: read(root, "data/tags.toml")?,
            en: read(root, "i18n/en.toml")?,
            ja: read(root, "i18n/ja.toml")?,
            issue_form: read(root, radar_core::guide::ISSUE_FORM_PATH).ok(),
            faq: read(root, radar_core::faq::FAQ_PATH).ok(),
        })
    }

    pub fn validate(&self) -> (Vec<Diagnostic>, Project) {
        let (mut diags, project) = validate(&self.tools, &self.tags, &self.en, &self.ja);
        if let Some(form) = &self.issue_form {
            diags.extend(radar_core::check_issue_form(form, &project.taxonomy));
        }
        if let Some(faq) = &self.faq {
            diags.extend(radar_core::faq::check_faq(faq, &project.taxonomy));
        }
        (diags, project)
    }

    /// Validate and return the project, or all errors.
    pub fn validated(&self) -> Result<Project, Vec<Diagnostic>> {
        let (diags, project) = self.validate();
        if has_errors(&diags) {
            Err(diags)
        } else {
            Ok(project)
        }
    }
}

fn is_gtm_id(id: &str) -> bool {
    id.strip_prefix("GTM-").is_some_and(|r| {
        r.len() >= 4
            && r.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    })
}

/// The token Search Console shows for the HTML-tag method: 10 to 100 letters, digits, `-` or `_`.
fn is_site_verification(v: &str) -> bool {
    (10..=100).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn load_site_config(root: &Path) -> Result<SiteConfig> {
    let f = read(root, "site.toml")?;
    let site =
        SiteConfig::parse(&f.text).map_err(|e| anyhow::anyhow!("site.toml: {}", e.message()))?;
    if let Some(id) = &site.gtm_id
        && !is_gtm_id(id)
    {
        anyhow::bail!("site.toml: gtm_id must look like GTM-XXXXXXX (got {id:?})");
    }
    if let Some(v) = &site.google_site_verification
        && !is_site_verification(v)
    {
        anyhow::bail!(
            "site.toml: google_site_verification must be 10-100 letters, digits, '-' or '_' (got {v:?})"
        );
    }
    Ok(site)
}
