//! `radar add` against a mock server (no real site or GitHub is contacted).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use radar_cli::add::{self, Config};
use serde_json::json;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    dir
}

fn cfg(server: &MockServer) -> Config {
    Config {
        timeout: Duration::from_secs(5),
        github_api: server.uri(),
        github_token: None,
        today: "2027-03-04".into(),
    }
}

async fn github_repo(server: &MockServer, full: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/{full}")))
        .and(header_exists("user-agent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn html(server: &MockServer, p: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(server)
        .await;
}

/// The draft keeps every comment and has the keys of the canonical form.
fn assert_canonical(text: &str) {
    assert_eq!(radar_cli::tool_file::canonical_text(text).unwrap(), text);
}

#[tokio::test]
async fn github_repo_with_homepage_uses_it_and_reads_the_license() {
    let server = MockServer::start().await;
    github_repo(
        &server,
        "acme/cool-agent",
        json!({
            "full_name": "acme/cool-agent", "name": "cool-agent",
            "description": "An agent that is cool",
            "homepage": "https://cool.example/", "archived": false,
            "license": {"spdx_id": "Apache-2.0"}
        }),
    )
    .await;
    let root = fixture("add_homepage");
    let d = add::draft(
        &root,
        "https://github.com/acme/cool-agent",
        None,
        &cfg(&server),
    )
    .await
    .unwrap();
    assert_eq!(d.id, "cool-agent");
    assert_eq!(d.path, "data/tools/cool-agent.toml");
    assert!(d.warnings.is_empty());
    let t = &d.text;
    assert!(t.contains("name = \"Cool Agent\""), "{t}");
    assert!(t.contains("url = \"https://cool.example/\""), "{t}");
    assert!(t.contains("repo = \"acme/cool-agent\""), "{t}");
    assert!(t.contains("summary.en = \"An agent that is cool\""), "{t}");
    assert!(t.contains("summary.ja = \"\""), "{t}");
    assert!(t.contains("pricing = \"free\""), "{t}");
    assert!(t.contains("# free = no cost"), "{t}");
    assert!(t.contains("open_source = true"), "{t}");
    assert!(t.contains("platforms = []"), "{t}");
    assert!(t.contains("tags = []"), "{t}");
    assert!(t.contains("added = 2027-03-04"), "{t}");
    assert!(
        t.contains("#   category (1 to 3, required): coding-agent, sandbox, memory, gateway"),
        "{t}"
    );
    assert!(
        t.find("# Tags:").unwrap() < t.find("tags = []").unwrap(),
        "{t}"
    );
    assert_canonical(t);
    // It parses as a tool file, and validate rejects it until it is filled in.
    let tool = radar_core::Tool::parse("cool-agent", t).unwrap();
    assert_eq!(tool.file.repo.as_deref(), Some("acme/cool-agent"));
}

#[tokio::test]
async fn github_repo_without_homepage_or_osi_license_and_archived() {
    let server = MockServer::start().await;
    github_repo(
        &server,
        "acme/OpenThing",
        json!({
            "full_name": "acme/OpenThing", "name": "OpenThing",
            "description": null, "homepage": "", "archived": true,
            "license": {"spdx_id": "NOASSERTION"}
        }),
    )
    .await;
    let root = fixture("add_no_homepage");
    let d = add::draft(
        &root,
        "https://github.com/acme/OpenThing",
        None,
        &cfg(&server),
    )
    .await
    .unwrap();
    assert_eq!(d.id, "openthing");
    assert!(d.text.contains("name = \"OpenThing\""), "{}", d.text);
    assert!(
        d.text
            .contains("url = \"https://github.com/acme/OpenThing\"")
    );
    assert!(d.text.contains("summary.en = \"\""));
    assert!(d.text.contains("open_source = false"));
    assert_eq!(d.warnings.len(), 1);
    assert!(d.warnings[0].contains("archived"), "{:?}", d.warnings);
}

#[tokio::test]
async fn id_option_overrides_the_generated_id_and_is_checked() {
    let server = MockServer::start().await;
    github_repo(
        &server,
        "acme/x",
        json!({"full_name": "acme/x", "name": "x", "license": {"spdx_id": "MIT"}}),
    )
    .await;
    let root = fixture("add_id");
    let d = add::draft(
        &root,
        "https://github.com/acme/x",
        Some("my-x"),
        &cfg(&server),
    )
    .await
    .unwrap();
    assert_eq!(d.id, "my-x");
    let e = add::draft(
        &root,
        "https://github.com/acme/x",
        Some("My X"),
        &cfg(&server),
    )
    .await
    .unwrap_err();
    assert!(format!("{e:#}").contains("lowercase"), "{e:#}");
}

#[test]
fn ids_names_and_licenses() {
    assert_eq!(add::slugify("Claude Code"), "claude-code");
    assert_eq!(add::slugify("  E2B (Sandbox)! "), "e2b-sandbox");
    assert_eq!(add::slugify("日本語"), "");
    assert_eq!(add::display_name("claude-code"), "Claude Code");
    assert_eq!(add::display_name("OpenHands"), "OpenHands");
    assert_eq!(add::display_name("vibe_kanban"), "Vibe Kanban");
    for ok in [
        "MIT",
        "Apache-2.0",
        "GPL-3.0-only",
        "AGPL-3.0-or-later",
        "BSD-3-Clause",
        "MPL-2.0",
        "Unlicense",
    ] {
        assert!(add::is_osi_license(ok), "{ok}");
    }
    for no in [
        "NOASSERTION",
        "Other",
        "BUSL-1.1",
        "Elastic-2.0",
        "SSPL-1.0",
        "",
    ] {
        assert!(!add::is_osi_license(no), "{no}");
    }
    assert_eq!(
        add::parse_github_repo("https://github.com/a/b.git/"),
        Some(("a".into(), "b".into()))
    );
    assert_eq!(
        add::parse_github_repo("https://github.com/sponsors/x"),
        None
    );
    assert_eq!(add::parse_github_repo("https://github.com/a"), None);
    assert_eq!(add::parse_github_repo("https://gitlab.com/a/b"), None);
}

#[test]
fn og_tags_win_over_title_and_meta_description() {
    let p = add::parse_page(
        r#"<html><head><title>Fallback Title | Tagline</title>
        <meta property="og:site_name" content="  Real   Name ">
        <meta name="description" content="plain desc">
        <meta property="og:description" content="og desc">
        </head><body></body></html>"#,
    );
    assert_eq!(p.name.as_deref(), Some("Real Name"));
    assert_eq!(p.description.as_deref(), Some("og desc"));
    assert_eq!(p.repo, None);
    let p = add::parse_page(
        "<title>Foo - the best thing</title><meta name=description content='plain desc'>",
    );
    assert_eq!(p.name.as_deref(), Some("Foo"));
    assert_eq!(p.description.as_deref(), Some("plain desc"));
}

#[tokio::test]
async fn web_page_with_one_github_link_takes_the_repo_too() {
    let server = MockServer::start().await;
    html(
        &server,
        "/product",
        r#"<html><head><meta property="og:site_name" content="Shiny Tool">
        <meta property="og:description" content="Does shiny things"></head>
        <body><a href="https://github.com/shiny/tool">GitHub</a>
        <a href="https://github.com/shiny/tool/issues">Issues</a>
        <a href="https://github.com/sponsors/shiny">Sponsor</a></body></html>"#,
    )
    .await;
    github_repo(
        &server,
        "shiny/tool",
        json!({"full_name": "shiny/tool", "name": "tool", "description": "gh desc",
               "homepage": "https://elsewhere.example", "license": {"spdx_id": "MIT"}}),
    )
    .await;
    let root = fixture("add_page_repo");
    let url = format!("{}/product", server.uri());
    let d = add::draft(&root, &url, None, &cfg(&server)).await.unwrap();
    assert_eq!(d.id, "shiny-tool");
    let t = &d.text;
    assert!(t.contains("name = \"Shiny Tool\""), "{t}");
    assert!(t.contains(&format!("url = \"{url}\"")), "{t}");
    assert!(t.contains("repo = \"shiny/tool\""), "{t}");
    assert!(t.contains("summary.en = \"Does shiny things\""), "{t}");
    assert!(t.contains("open_source = true"), "{t}");
    assert_canonical(t);
}

#[tokio::test]
async fn web_page_with_two_github_links_or_none_leaves_repo_out() {
    let server = MockServer::start().await;
    html(
        &server,
        "/two",
        r#"<title>Two Repos</title><a href="https://github.com/a/one">1</a>
        <a href="https://github.com/b/two">2</a>"#,
    )
    .await;
    html(&server, "/none", "<title>No Repo</title>").await;
    let root = fixture("add_page_no_repo");
    for (p, id) in [("/two", "two-repos"), ("/none", "no-repo")] {
        let d = add::draft(&root, &format!("{}{p}", server.uri()), None, &cfg(&server))
            .await
            .unwrap();
        assert_eq!(d.id, id);
        assert!(!d.text.contains("repo ="), "{}", d.text);
        assert!(d.text.contains("open_source = false"));
        assert!(d.text.contains("summary.en = \"\""));
        assert!(d.warnings.iter().any(|w| w.contains("repo left out")));
    }
}

#[tokio::test]
async fn page_without_a_name_asks_for_an_id() {
    let server = MockServer::start().await;
    html(&server, "/bare", "<html><body>hi</body></html>").await;
    let root = fixture("add_bare");
    let url = format!("{}/bare", server.uri());
    let e = add::draft(&root, &url, None, &cfg(&server))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("--id"), "{e:#}");
    let d = add::draft(&root, &url, Some("bare"), &cfg(&server))
        .await
        .unwrap();
    assert!(d.text.contains("name = \"\""), "{}", d.text);
}

#[tokio::test]
async fn html_is_read_only_up_to_one_megabyte() {
    let server = MockServer::start().await;
    let big = format!(
        "<title>Big Page</title><!--{}--><meta property=\"og:site_name\" content=\"Too Late\">",
        "x".repeat(1024 * 1024)
    );
    html(&server, "/big", &big).await;
    let root = fixture("add_big");
    let d = add::draft(&root, &format!("{}/big", server.uri()), None, &cfg(&server))
        .await
        .unwrap();
    assert!(d.text.contains("name = \"Big Page\""), "{}", d.text);
}

#[tokio::test]
async fn duplicates_are_refused_with_the_file_that_has_them() {
    let server = MockServer::start().await;
    let root = fixture("add_dup");
    fs::write(
        root.join("data/tools/gamma.toml"),
        fs::read_to_string(root.join("data/tools/alpha.toml"))
            .unwrap()
            .replace("Alpha", "Gamma")
            .replace("https://alpha.example", "https://gamma.example")
            .replace("summary.en", "repo = \"Acme/Gamma\"\nsummary.en"),
    )
    .unwrap();
    // Same repo, different case: stopped before any request is made.
    let e = add::draft(&root, "https://github.com/acme/gamma", None, &cfg(&server))
        .await
        .unwrap_err();
    let m = format!("{e:#}");
    assert!(
        m.contains("data/tools/gamma.toml") && m.contains("repo"),
        "{m}"
    );
    // Same url (trailing slash and case do not matter).
    let e = add::draft(&root, "https://Alpha.example/", None, &cfg(&server))
        .await
        .unwrap_err();
    let m = format!("{e:#}");
    assert!(
        m.contains("data/tools/alpha.toml") && m.contains("url"),
        "{m}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn homepage_that_is_already_listed_is_a_duplicate() {
    let server = MockServer::start().await;
    github_repo(
        &server,
        "acme/fork",
        json!({"full_name": "acme/fork", "name": "fork", "homepage": "https://alpha.example"}),
    )
    .await;
    let root = fixture("add_dup_homepage");
    let e = add::draft(&root, "https://github.com/acme/fork", None, &cfg(&server))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("data/tools/alpha.toml"), "{e:#}");
}

#[tokio::test]
async fn missing_repository_and_existing_file_are_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/ghost"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    github_repo(
        &server,
        "acme/alpha-two",
        json!({"full_name": "acme/alpha-two", "name": "alpha-two"}),
    )
    .await;
    let root = fixture("add_errors");
    let e = add::draft(&root, "https://github.com/acme/ghost", None, &cfg(&server))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("not found"), "{e:#}");
    // An existing file is never overwritten.
    let e = add::draft(
        &root,
        "https://github.com/acme/alpha-two",
        Some("alpha"),
        &cfg(&server),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{e:#}").contains("data/tools/alpha.toml already exists"),
        "{e:#}"
    );
    let e = add::draft(&root, "ftp://x.example", None, &cfg(&server))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("http"), "{e:#}");
}

#[tokio::test]
async fn token_is_sent_when_given() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/t"))
        .and(wiremock::matchers::header("authorization", "Bearer secret"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"full_name": "acme/t", "name": "t"})),
        )
        .mount(&server)
        .await;
    let root = fixture("add_token");
    let mut c = cfg(&server);
    c.github_token = Some("secret".into());
    add::draft(&root, "https://github.com/acme/t", None, &c)
        .await
        .unwrap();
    assert!(
        add::draft(&root, "https://github.com/acme/t", None, &cfg(&server))
            .await
            .is_err()
    );
}

#[test]
fn binary_refuses_duplicates_without_touching_the_files() {
    let root = fixture("add_cli_dup");
    let alpha = root.join("data/tools/alpha.toml");
    let before = fs::read_to_string(&alpha).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_radar"))
        .arg("--root")
        .arg(&root)
        .args(["add", "https://alpha.example", "--dry-run"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("data/tools/alpha.toml"), "{err}");
    assert_eq!(fs::read_to_string(&alpha).unwrap(), before);
}

#[tokio::test]
async fn a_draft_fails_validate_until_it_is_filled_in() {
    let server = MockServer::start().await;
    github_repo(
        &server,
        "acme/fresh",
        json!({"full_name": "acme/fresh", "name": "fresh", "description": "Fresh", "license": {"spdx_id": "MIT"}}),
    )
    .await;
    let root = fixture("add_validate");
    let d = add::draft(&root, "https://github.com/acme/fresh", None, &cfg(&server))
        .await
        .unwrap();
    fs::write(root.join(&d.path), &d.text).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_radar"))
        .arg("--root")
        .arg(&root)
        .arg("validate")
        .output()
        .unwrap();
    assert!(!o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("summary.ja is missing or empty"), "{out}");
    assert!(out.contains("platforms must not be empty"), "{out}");
    assert!(out.contains("facet \"category\" needs at least 1"), "{out}");
}
