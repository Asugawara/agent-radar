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
        })
    }

    pub fn validate(&self) -> (Vec<Diagnostic>, Project) {
        let (mut diags, project) = validate(&self.tools, &self.tags, &self.en, &self.ja);
        if let Some(form) = &self.issue_form {
            diags.extend(radar_core::check_issue_form(form, &project.taxonomy));
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

pub fn load_site_config(root: &Path) -> Result<SiteConfig> {
    let f = read(root, "site.toml")?;
    SiteConfig::parse(&f.text).map_err(|e| anyhow::anyhow!("site.toml: {}", e.message()))
}
