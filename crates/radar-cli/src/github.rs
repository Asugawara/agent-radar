//! GitHub access: repo existence (REST) and metadata for `radar enrich`
//! (GraphQL, 50 repositories per request through aliases).

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use radar_core::RepoMeta;
use serde_json::{Value, json};

pub const API_BASE: &str = "https://api.github.com";
pub const BATCH: usize = 50;

pub fn user_agent() -> String {
    format!(
        "agent-radar-linkcheck/{} (+https://github.com/Asugawara/agent-radar)",
        env!("CARGO_PKG_VERSION")
    )
}

/// `owner/name` -> `(owner, name)`.
pub fn split_repo(repo: &str) -> Option<(&str, &str)> {
    let (o, n) = repo.split_once('/')?;
    (!o.is_empty() && !n.is_empty() && !n.contains('/')).then_some((o, n))
}

fn graphql_query(n: usize) -> String {
    let vars: Vec<String> = (0..n)
        .map(|i| format!("$o{i}: String!, $n{i}: String!"))
        .collect();
    let fields: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "r{i}: repository(owner: $o{i}, name: $n{i}) \
                 {{ stargazerCount pushedAt isArchived licenseInfo {{ spdxId }} }}"
            )
        })
        .collect();
    format!("query({}) {{ {} }}", vars.join(", "), fields.join(" "))
}

fn graphql_request(repos: &[&str]) -> Result<Value> {
    let mut variables = serde_json::Map::new();
    for (i, repo) in repos.iter().enumerate() {
        let (o, n) = split_repo(repo).with_context(|| format!("invalid repo \"{repo}\""))?;
        variables.insert(format!("o{i}"), json!(o));
        variables.insert(format!("n{i}"), json!(n));
    }
    Ok(json!({ "query": graphql_query(repos.len()), "variables": variables }))
}

/// Parse a GraphQL response for `repos` (alias `r<i>` = `repos[i]`).
/// Repositories that no longer exist are returned in the second value.
pub fn parse_graphql(
    repos: &[&str],
    body: &Value,
) -> Result<(BTreeMap<String, RepoMeta>, Vec<String>)> {
    let data = body.get("data").filter(|d| !d.is_null());
    let Some(data) = data else {
        bail!(
            "GraphQL response has no data: {}",
            body.get("errors").unwrap_or(body)
        );
    };
    let mut out = BTreeMap::new();
    let mut missing = Vec::new();
    for (i, repo) in repos.iter().enumerate() {
        match data.get(format!("r{i}")) {
            Some(Value::Object(o)) => {
                let stars = o
                    .get("stargazerCount")
                    .and_then(Value::as_u64)
                    .with_context(|| format!("{repo}: stargazerCount missing"))?;
                let pushed_at = o
                    .get("pushedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let archived = o
                    .get("isArchived")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let license = o
                    .get("licenseInfo")
                    .and_then(|l| l.get("spdxId"))
                    .and_then(Value::as_str)
                    .filter(|s| *s != "NOASSERTION")
                    .map(str::to_string);
                out.insert(
                    (*repo).to_string(),
                    RepoMeta {
                        stars,
                        pushed_at,
                        archived,
                        license,
                    },
                );
            }
            _ => missing.push((*repo).to_string()),
        }
    }
    Ok((out, missing))
}

/// Fetch metadata for all `repos` (batches of [`BATCH`]). Returns the map and
/// the repositories GitHub does not know.
pub async fn fetch_repo_meta(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repos: &[String],
) -> Result<(BTreeMap<String, RepoMeta>, Vec<String>)> {
    let mut all = BTreeMap::new();
    let mut missing = Vec::new();
    for chunk in repos.chunks(BATCH) {
        let names: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let res = client
            .post(endpoint)
            .bearer_auth(token)
            .json(&graphql_request(&names)?)
            .send()
            .await
            .context("GraphQL request failed")?;
        let status = res.status();
        if !status.is_success() {
            bail!("GraphQL request returned {status}");
        }
        let body: Value = res.json().await.context("GraphQL response is not JSON")?;
        let (found, gone) = parse_graphql(&names, &body)?;
        all.extend(found);
        missing.extend(gone);
    }
    Ok((all, missing))
}

/// `radar enrich`.
pub async fn enrich(root: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .context("GITHUB_TOKEN is not set")?;
    let sources = crate::project::Sources::load(root)?;
    let project = sources
        .validated()
        .map_err(|d| anyhow::anyhow!("{} validation error(s); run `radar validate`", d.len()))?;
    let mut repos: Vec<String> = project
        .tools
        .iter()
        .filter_map(|t| t.file.repo.clone())
        .collect();
    repos.sort();
    repos.dedup();
    let client = reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let endpoint = format!("{API_BASE}/graphql");
    let (meta, missing) = fetch_repo_meta(&client, &endpoint, &token, &repos).await?;
    for m in &missing {
        eprintln!("warning: repository {m} not found on GitHub");
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&meta)? + "\n")
        .with_context(|| format!("write {}", out.display()))?;
    println!(
        "enriched {} of {} repositories -> {}",
        meta.len(),
        repos.len(),
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": {
        "r0": {"stargazerCount": 1234, "pushedAt": "2026-10-30T12:00:00Z", "isArchived": false,
               "licenseInfo": {"spdxId": "Apache-2.0"}},
        "r1": {"stargazerCount": 5, "pushedAt": "2020-01-01T00:00:00Z", "isArchived": true,
               "licenseInfo": null},
        "r2": null,
        "r3": {"stargazerCount": 7, "pushedAt": "2026-01-01T00:00:00Z", "isArchived": false,
               "licenseInfo": {"spdxId": "NOASSERTION"}}
      },
      "errors": [{"type": "NOT_FOUND", "path": ["r2"], "message": "Could not resolve"}]
    }"#;

    #[test]
    fn parses_aliases_licenses_and_missing_repos() {
        let body: Value = serde_json::from_str(FIXTURE).unwrap();
        let repos = ["a/one", "b/two", "c/gone", "d/four"];
        let (meta, missing) = parse_graphql(&repos, &body).unwrap();
        assert_eq!(missing, vec!["c/gone"]);
        assert_eq!(meta["a/one"].stars, 1234);
        assert_eq!(meta["a/one"].license.as_deref(), Some("Apache-2.0"));
        assert_eq!(meta["a/one"].pushed_at, "2026-10-30T12:00:00Z");
        assert!(meta["b/two"].archived);
        assert_eq!(meta["b/two"].license, None);
        assert_eq!(meta["d/four"].license, None);
    }

    #[test]
    fn output_matches_the_site_contract() {
        let body: Value = serde_json::from_str(FIXTURE).unwrap();
        let (meta, _) = parse_graphql(&["a/one", "b/two", "c/gone", "d/four"], &body).unwrap();
        let json = serde_json::to_string(&meta).unwrap();
        let back: BTreeMap<String, RepoMeta> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn response_without_data_is_an_error() {
        let body = json!({"errors": [{"message": "Bad credentials"}]});
        assert!(parse_graphql(&["a/b"], &body).is_err());
    }

    #[test]
    fn request_has_one_alias_per_repo() {
        let req = graphql_request(&["a/b", "c/d"]).unwrap();
        let q = req["query"].as_str().unwrap();
        assert!(q.contains("r0: repository(owner: $o0, name: $n0)"));
        assert!(q.contains("r1: repository(owner: $o1, name: $n1)"));
        assert_eq!(req["variables"]["o1"], "c");
        assert_eq!(req["variables"]["n1"], "d");
        assert!(graphql_request(&["nonsense"]).is_err());
    }
}
