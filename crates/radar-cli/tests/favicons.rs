//! `radar favicons` against a mock server (no real site is contacted).

use std::fs;
use std::path::{Path, PathBuf};

use radar_cli::favicons::{self, Config, Manifest};
use time::OffsetDateTime;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
const ICO: &[u8] = &[0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0];

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

/// The `basic` fixture with its tools replaced by `tools` (id, url, extra toml).
fn fixture(name: &str, tools: &[(&str, String, String)]) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    let dir_tools = dir.join("data/tools");
    fs::remove_dir_all(&dir_tools).unwrap();
    fs::create_dir_all(&dir_tools).unwrap();
    for (id, url, extra) in tools {
        let text = format!(
            "name = \"{id}\"\nurl = \"{url}\"\nsummary.en = \"e\"\nsummary.ja = \"j\"\n\
             pricing = \"free\"\nopen_source = true\nplatforms = [\"linux\"]\n\
             tags = [\"coding-agent\", \"sandbox\", \"cli\", \"parallel\"]\nadded = 2026-11-02\n{extra}"
        );
        fs::write(dir_tools.join(format!("{id}.toml")), text).unwrap();
    }
    dir
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

async fn page(server: &MockServer, p: &str, html: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_raw(html, "text/html; charset=utf-8"))
        .mount(server)
        .await;
}

async fn asset(server: &MockServer, p: &str, ct: &str, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, ct))
        .mount(server)
        .await;
}

fn cfg(server: &MockServer) -> Config {
    Config {
        github_base: server.uri(),
        ..Config::new(7)
    }
}

async fn hits(server: &MockServer, p: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == p)
        .count()
}

#[tokio::test]
async fn fetches_validates_and_caches() {
    let server = MockServer::start().await;
    let b = server.uri();
    let root = fixture(
        "favicons-flow",
        &[
            ("a-png", format!("{b}/a/"), String::new()),
            ("b-notimg", format!("{b}/b/"), String::new()),
            ("c-big", format!("{b}/c/"), String::new()),
            (
                "d-manual",
                format!("{b}/d/"),
                format!("favicon = \"{b}/manual.png\"\n"),
            ),
            (
                "e-gh",
                "https://github.com/acme/thing".into(),
                String::new(),
            ),
            ("f-default", format!("{b}/f/"), String::new()),
        ],
    );

    page(
        &server,
        "/a/",
        r#"<link rel="icon" sizes="16x16" href="/icons/a16.png"><link rel="icon" type="image/png" sizes="32x32" href="/icons/a.png">"#,
    )
    .await;
    asset(&server, "/icons/a.png", "image/png", PNG.to_vec()).await;
    // Smaller icons are only fallbacks; the 32px one wins.
    asset(&server, "/icons/a16.png", "image/png", PNG.to_vec()).await;

    page(
        &server,
        "/b/",
        r#"<link rel="icon" href="/icons/b.png" sizes="32x32">"#,
    )
    .await;
    // Content-Type says image, bytes are HTML -> rejected; /favicon.ico missing.
    asset(&server, "/icons/b.png", "text/html", b"<html>".to_vec()).await;

    page(
        &server,
        "/c/",
        r#"<link rel="icon" href="/icons/c.png" sizes="32x32">"#,
    )
    .await;
    let mut big = PNG.to_vec();
    big.resize(300 * 1024, 0);
    asset(&server, "/icons/c.png", "image/png", big).await;

    page(
        &server,
        "/d/",
        r#"<link rel="icon" href="/icons/other.png" sizes="32x32">"#,
    )
    .await;
    asset(&server, "/manual.png", "image/png", PNG.to_vec()).await;
    asset(&server, "/icons/other.png", "image/png", PNG.to_vec()).await;

    Mock::given(method("GET"))
        .and(path("/acme.png"))
        .and(query_param("size", "64"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(PNG.to_vec(), "image/png"))
        .mount(&server)
        .await;

    page(&server, "/f/", "<html><head></head></html>").await;
    asset(&server, "/favicon.ico", "image/x-icon", ICO.to_vec()).await;

    let out = root.join("out");
    let r = favicons::fetch_all(&root, &out, &cfg(&server), now())
        .await
        .unwrap();
    let m: Manifest = favicons::read_manifest(&out).unwrap();

    let a = &m["a-png"];
    assert_eq!(a.file.as_deref(), Some("a-png.png"));
    assert_eq!(a.kind.as_deref(), Some("html"));
    assert_eq!(
        a.source.as_deref(),
        Some(format!("{b}/icons/a.png").as_str())
    );
    assert!(out.join("a-png.png").is_file());
    assert_eq!(hits(&server, "/icons/a16.png").await, 0);

    // b and c reach /favicon.ico (shared by every tool of this server) only
    // because their rejected candidates did not stop the chain.
    for id in ["b-notimg", "c-big"] {
        assert_eq!(m[id].kind.as_deref(), Some("default"), "{id}: {:?}", m[id]);
    }

    let d = &m["d-manual"];
    assert_eq!(d.kind.as_deref(), Some("manual"));
    assert_eq!(hits(&server, "/icons/other.png").await, 0);

    let e = &m["e-gh"];
    assert_eq!(e.kind.as_deref(), Some("github"));
    assert_eq!(e.file.as_deref(), Some("e-gh.png"));

    let f = &m["f-default"];
    assert_eq!(f.file.as_deref(), Some("f-default.ico"));
    assert_eq!(f.kind.as_deref(), Some("default"));
    assert_eq!(r.fetched, 6);
    assert_eq!(r.failed, 0);

    // Second run within max-age: nothing is fetched again.
    let before = server.received_requests().await.unwrap().len();
    let r = favicons::fetch_all(&root, &out, &cfg(&server), now() + time::Duration::days(3))
        .await
        .unwrap();
    assert_eq!((r.cached, r.fetched), (6, 0));
    assert_eq!(server.received_requests().await.unwrap().len(), before);

    // After max-age they are fetched again.
    let r = favicons::fetch_all(&root, &out, &cfg(&server), now() + time::Duration::days(8))
        .await
        .unwrap();
    assert_eq!((r.cached, r.fetched), (0, 6));
}

#[tokio::test]
async fn rejections_are_recorded_and_do_not_fail_the_command() {
    let server = MockServer::start().await;
    let b = server.uri();
    let root = fixture(
        "favicons-fail",
        &[
            ("b-notimg", format!("{b}/b/"), String::new()),
            ("c-big", format!("{b}/c/"), String::new()),
            ("g-down", "http://127.0.0.1:9/".into(), String::new()),
        ],
    );
    page(
        &server,
        "/b/",
        r#"<link rel="icon" href="/icons/b.png" sizes="32x32">"#,
    )
    .await;
    asset(&server, "/icons/b.png", "text/html", b"<html>".to_vec()).await;
    page(
        &server,
        "/c/",
        r#"<link rel="icon" href="/icons/c.png" sizes="32x32">"#,
    )
    .await;
    let mut big = PNG.to_vec();
    big.resize(300 * 1024, 0);
    asset(&server, "/icons/c.png", "image/png", big).await;
    // No /favicon.ico anywhere: wiremock answers 404.

    let out = root.join("out");
    let r = favicons::fetch_all(&root, &out, &cfg(&server), now())
        .await
        .unwrap();
    assert_eq!((r.fetched, r.failed), (0, 3));
    let m = favicons::read_manifest(&out).unwrap();
    assert!(m["b-notimg"].file.is_none());
    assert!(
        m["b-notimg"]
            .error
            .as_deref()
            .unwrap()
            .contains("not an image")
    );
    assert!(
        m["c-big"]
            .error
            .as_deref()
            .unwrap()
            .contains("larger than 256 KB")
    );
    assert!(m["g-down"].error.is_some());
    assert!(!out.join("b-notimg.png").exists());
}
