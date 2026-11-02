//! `radar check-links`: check every target, advance the state machine and
//! apply the results to `data/tools/*.toml`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::task::JoinSet;

use super::http::Checker;
use super::state::{Entry, Health, Outcome, State, step};
use crate::project::Sources;
use crate::tool_file;

/// What is checked for a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    Url,
    Link(String),
    Repo,
}

#[derive(Debug, Clone)]
struct Target {
    /// Key in `health.json`.
    key: String,
    tool: String,
    role: Role,
}

pub struct Options {
    pub root: PathBuf,
    pub state: PathBuf,
    pub dry_run: bool,
    pub now: OffsetDateTime,
}

#[derive(Debug, Default)]
pub struct Report {
    pub dry_run: bool,
    pub checked: usize,
    /// (tool, link key, url, reason)
    pub removed_links: Vec<(String, String, String, String)>,
    /// (tool, reason)
    pub hidden: Vec<(String, String)>,
    pub restored: Vec<String>,
    /// (tool, repo)
    pub removed_repos: Vec<(String, String)>,
    /// (target, result)
    pub undecided: Vec<(String, String)>,
    /// Targets currently suspected dead: (target, result, dead since)
    pub suspect: Vec<(String, String, String)>,
    pub unchecked: usize,
}

impl Report {
    pub fn changed_data(&self) -> bool {
        !(self.removed_links.is_empty()
            && self.hidden.is_empty()
            && self.restored.is_empty()
            && self.removed_repos.is_empty())
    }

    pub fn to_markdown(&self) -> String {
        let (would, dry) = if self.dry_run {
            ("would be ", " (dry run: nothing written)")
        } else {
            ("", "")
        };
        let mut s = format!("## Link check{dry}\n\nChecked {} targets", self.checked);
        if self.unchecked > 0 {
            s += &format!(", {} not checked (x.com / twitter.com)", self.unchecked);
        }
        s += ".\n\n";
        let mut section = |title: &str, lines: Vec<String>| {
            if lines.is_empty() {
                return;
            }
            s += &format!("### {title}\n\n");
            for l in lines {
                s += &format!("- {l}\n");
            }
            s += "\n";
        };
        section(
            &format!("Links {would}removed"),
            self.removed_links
                .iter()
                .map(|(t, k, u, r)| format!("`{t}` links.{k}: {u} ({r})"))
                .collect(),
        );
        section(
            &format!("Tools {would}hidden"),
            self.hidden
                .iter()
                .map(|(t, r)| format!("`{t}`: {r}"))
                .collect(),
        );
        section(
            &format!("Tools {would}restored"),
            self.restored.iter().map(|t| format!("`{t}`")).collect(),
        );
        section(
            &format!("Repos {would}removed from tools"),
            self.removed_repos
                .iter()
                .map(|(t, r)| format!("`{t}`: {r}"))
                .collect(),
        );
        section(
            "Suspected dead (waiting for 24 h of consecutive failures)",
            self.suspect
                .iter()
                .map(|(t, r, since)| format!("{t}: {r} (since {since})"))
                .collect(),
        );
        section(
            "Undecided (not counted)",
            self.undecided
                .iter()
                .map(|(t, r)| format!("{t}: {r}"))
                .collect(),
        );
        if !self.changed_data() {
            s += "No data changes.\n";
        }
        s
    }
}

fn targets(project: &radar_core::Project) -> Vec<Target> {
    let mut out = Vec::new();
    for tool in &project.tools {
        let f = &tool.file;
        out.push(Target {
            key: f.url.clone(),
            tool: tool.id.clone(),
            role: Role::Url,
        });
        if f.is_hidden() {
            continue; // only the url decides whether a hidden tool comes back
        }
        for (k, u) in &f.links {
            out.push(Target {
                key: u.clone(),
                tool: tool.id.clone(),
                role: Role::Link(k.clone()),
            });
        }
        if let Some(r) = &f.repo {
            out.push(Target {
                key: format!("repo:{r}"),
                tool: tool.id.clone(),
                role: Role::Repo,
            });
        }
    }
    out
}

fn date_of(t: &str) -> String {
    t.get(..10).unwrap_or(t).to_string()
}

pub async fn check_links(opts: &Options, checker: Arc<Checker>) -> Result<Report> {
    let sources = Sources::load(&opts.root)?;
    let project = sources.validated().map_err(|d| {
        for x in &d {
            eprintln!("{x}");
        }
        anyhow::anyhow!("validation failed; run `radar validate`")
    })?;
    let targets = targets(&project);

    let mut health: Health = match std::fs::read_to_string(&opts.state) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("parse {}", opts.state.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Health::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", opts.state.display())),
    };

    // Each distinct key is checked once.
    let keys: BTreeSet<&str> = targets.iter().map(|t| t.key.as_str()).collect();
    let mut set = JoinSet::new();
    for key in keys {
        let key = key.to_string();
        let checker = checker.clone();
        set.spawn(async move {
            let outcome = match key.strip_prefix("repo:") {
                Some(repo) => checker.check_repo(repo).await,
                None => checker.check_url(&key).await,
            };
            (key, outcome)
        });
    }
    let mut outcomes: BTreeMap<String, Outcome> = BTreeMap::new();
    while let Some(res) = set.join_next().await {
        let (key, outcome) = res?;
        outcomes.insert(key, outcome);
    }

    // Advance the state machine once per key.
    let mut transitions: BTreeMap<String, (State, State)> = BTreeMap::new();
    let current: BTreeSet<&str> = targets.iter().map(|t| t.key.as_str()).collect();
    health.retain(|k, _| current.contains(k.as_str()));
    for (key, outcome) in &outcomes {
        let entry = health.entry(key.clone()).or_default();
        if let Some(t) = step(entry, outcome, opts.now) {
            transitions.insert(key.clone(), t);
        }
    }

    let mut report = Report {
        dry_run: opts.dry_run,
        checked: outcomes.len(),
        ..Report::default()
    };
    report.unchecked = outcomes
        .values()
        .filter(|o| **o == Outcome::Unchecked)
        .count();
    for (key, o) in &outcomes {
        if let Outcome::Undecided(r) = o {
            report.undecided.push((key.clone(), r.clone()));
        }
        let e = &health[key];
        if e.state == State::Suspect {
            report.suspect.push((
                key.clone(),
                e.last_dead_reason.clone().unwrap_or_default(),
                e.first_dead_at.as_deref().map(date_of).unwrap_or_default(),
            ));
        }
    }

    // Apply per tool.
    let mut by_tool: BTreeMap<&str, Vec<&Target>> = BTreeMap::new();
    for t in &targets {
        by_tool.entry(t.tool.as_str()).or_default().push(t);
    }
    let today = opts.now.date().to_string();
    for (tool_id, ts) in by_tool {
        let tool = project.tools.iter().find(|t| t.id == tool_id).unwrap();
        let path = opts.root.join(
            project
                .tool_paths
                .get(tool_id)
                .map(String::as_str)
                .unwrap_or_default(),
        );
        let mut doc = tool_file::read_doc(&path)?;
        let mut changed = false;
        let mut hidden_now = tool.file.is_hidden();
        for t in ts {
            let e: &Entry = &health[&t.key];
            match &t.role {
                Role::Link(k) if e.state == State::Hidden => {
                    if tool_file::remove_link(&mut doc, k) {
                        changed = true;
                        report.removed_links.push((
                            tool_id.into(),
                            k.clone(),
                            t.key.clone(),
                            e.last_dead_reason.clone().unwrap_or_default(),
                        ));
                    }
                }
                Role::Repo if e.state == State::Hidden => {
                    if doc.remove("repo").is_some() {
                        changed = true;
                        report
                            .removed_repos
                            .push((tool_id.into(), t.key.trim_start_matches("repo:").into()));
                    }
                }
                Role::Url if e.state == State::Hidden && !hidden_now => {
                    let reason = format!(
                        "url: {} (since {})",
                        e.last_dead_reason.as_deref().unwrap_or("dead"),
                        e.first_dead_at
                            .as_deref()
                            .map(date_of)
                            .unwrap_or(today.clone()),
                    );
                    tool_file::set_hidden(&mut doc, &reason, &today);
                    hidden_now = true;
                    changed = true;
                    report.hidden.push((tool_id.into(), reason));
                }
                Role::Url
                    if hidden_now
                        && transitions.get(&t.key) == Some(&(State::Recovering, State::Active))
                        && tool
                            .file
                            .hidden_reason
                            .as_deref()
                            .is_some_and(|r| r.starts_with("url: ")) =>
                {
                    tool_file::clear_hidden(&mut doc);
                    hidden_now = false;
                    changed = true;
                    report.restored.push(tool_id.into());
                }
                _ => {}
            }
        }
        if changed && !opts.dry_run {
            tool_file::write_doc(&path, &mut doc)?;
        }
    }

    if !opts.dry_run {
        if let Some(dir) = opts.state.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&opts.state, serde_json::to_string_pretty(&health)? + "\n")
            .with_context(|| format!("write {}", opts.state.display()))?;
    }
    Ok(report)
}

/// CLI entry: builds the runtime and prints / appends the summary.
pub fn run_cli(
    root: &Path,
    state: &Path,
    dry_run: bool,
    now: Option<OffsetDateTime>,
) -> Result<()> {
    let opts = Options {
        root: root.to_path_buf(),
        state: state.to_path_buf(),
        dry_run,
        now: now.unwrap_or_else(OffsetDateTime::now_utc),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let checker = Arc::new(Checker::new(super::http::Config::default())?);
    let report = rt.block_on(check_links(&opts, checker))?;
    let md = report.to_markdown();
    print!("{md}");
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY")
        && !path.is_empty()
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?;
        writeln!(f, "{md}")?;
    }
    Ok(())
}

pub fn parse_now(s: &str) -> Result<OffsetDateTime, String> {
    OffsetDateTime::parse(s, &Rfc3339).map_err(|e| format!("not an RFC 3339 time: {e}"))
}
