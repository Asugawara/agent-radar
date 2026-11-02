//! Whole-pipeline test of `check-links`: a mock server stands in for the web
//! and the clock is injected.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use radar_cli::health::http::{Checker, Config};
use radar_cli::health::{Options, check_links};
use time::OffsetDateTime;
use wiremock::matchers::path;
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

fn fixture(name: &str, base: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    let tools = dir.join("data/tools");
    let alpha = fs::read_to_string(tools.join("alpha.toml")).unwrap();
    let alpha = alpha
        .replace("https://alpha.example", &format!("{base}/alpha"))
        .replace("parallel\"]", "parallel\"]\nrepo = \"o/alpha\"")
        + &format!("\n[links]\ndocs = \"{base}/docs\"\nblog = \"{base}/blog\"\n");
    // `repo` must precede the arrays only for taste; toml_edit does not care.
    fs::write(tools.join("alpha.toml"), alpha).unwrap();
    let beta = fs::read_to_string(tools.join("beta.toml")).unwrap();
    fs::write(
        tools.join("beta.toml"),
        beta.replace("https://beta.example", &format!("{base}/beta")),
    )
    .unwrap();
    dir
}

async fn serve(server: &MockServer, alpha: u16, docs: u16, repo: u16) {
    server.reset().await;
    for (p, code) in [
        ("/alpha", alpha),
        ("/docs", docs),
        ("/blog", 200),
        ("/beta", 200),
    ] {
        Mock::given(path(p))
            .respond_with(ResponseTemplate::new(code))
            .mount(server)
            .await;
    }
    Mock::given(path("/repos/o/alpha"))
        .respond_with(ResponseTemplate::new(repo))
        .mount(server)
        .await;
}

fn at(hours: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap() + time::Duration::hours(hours)
}

struct Run<'a> {
    root: &'a Path,
    server: &'a MockServer,
}

impl Run<'_> {
    async fn go(&self, hours: i64, dry_run: bool) -> radar_cli::health::Report {
        let checker = Checker::new(Config {
            github_api: self.server.uri(),
            github_token: None,
            ..Config::default()
        })
        .unwrap();
        let opts = Options {
            root: self.root.to_path_buf(),
            state: self.root.join("health.json"),
            dry_run,
            now: at(hours),
        };
        check_links(&opts, Arc::new(checker)).await.unwrap()
    }
    fn alpha(&self) -> String {
        fs::read_to_string(self.root.join("data/tools/alpha.toml")).unwrap()
    }
}

#[tokio::test]
async fn hidden_after_24h_and_restored_after_three_good_checks() {
    let server = MockServer::start().await;
    let root = fixture("health-flow", &server.uri());
    let run = Run {
        root: &root,
        server: &server,
    };
    let original = run.alpha();

    // Everything about alpha is dead; the first dead result only makes it suspect.
    serve(&server, 404, 404, 404).await;
    let r = run.go(0, false).await;
    assert!(!r.changed_data());
    assert_eq!(run.alpha(), original);
    assert_eq!(r.suspect.len(), 3, "{}", r.to_markdown());

    // Dry run at +25h reports but writes nothing (neither tools nor state).
    let state_before = fs::read_to_string(root.join("health.json")).unwrap();
    let r = run.go(25, true).await;
    assert_eq!(r.hidden.len(), 1);
    assert!(r.to_markdown().contains("would be hidden"));
    assert_eq!(run.alpha(), original);
    assert_eq!(
        fs::read_to_string(root.join("health.json")).unwrap(),
        state_before
    );

    // Still dead 12 h after the first failure: not hidden yet.
    let r = run.go(12, false).await;
    assert!(!r.changed_data());
    assert_eq!(run.alpha(), original);

    // 25 h after the first failure: hidden, dead links and repo dropped.
    let r = run.go(25, false).await;
    assert_eq!(r.hidden.len(), 1, "{}", r.to_markdown());
    assert_eq!(r.removed_links.len(), 1);
    assert_eq!(r.removed_repos.len(), 1);
    let text = run.alpha();
    assert!(text.contains("status = \"hidden\""), "{text}");
    assert!(
        text.contains("hidden_reason = \"url: 404 (since 2027-01-15)\""),
        "{text}"
    );
    assert!(!text.contains("docs ="));
    assert!(text.contains("blog ="), "healthy link stays");
    assert!(!text.contains("repo ="));

    // Back up: one good check starts recovery, three in a row restore.
    serve(&server, 200, 200, 200).await;
    for h in [26, 27] {
        let r = run.go(h, false).await;
        assert!(r.restored.is_empty());
        assert!(run.alpha().contains("status = \"hidden\""), "hour {h}");
    }
    let r = run.go(28, false).await;
    assert_eq!(r.restored, vec!["alpha".to_string()]);
    let text = run.alpha();
    assert!(!text.contains("status"), "{text}");
    assert!(!text.contains("hidden_"), "{text}");

    // The files stay valid and canonical for the other commands.
    let sources = radar_cli::project::Sources::load(&root).unwrap();
    assert!(sources.validated().is_ok());
}

#[tokio::test]
async fn a_recovering_tool_that_dies_again_stays_hidden() {
    let server = MockServer::start().await;
    let root = fixture("health-relapse", &server.uri());
    let run = Run {
        root: &root,
        server: &server,
    };
    serve(&server, 410, 200, 200).await;
    run.go(0, false).await;
    run.go(24, false).await;
    assert!(run.alpha().contains("url: 410"));
    serve(&server, 200, 200, 200).await;
    run.go(25, false).await;
    run.go(26, false).await;
    serve(&server, 410, 200, 200).await;
    run.go(27, false).await;
    serve(&server, 200, 200, 200).await;
    run.go(28, false).await;
    run.go(29, false).await;
    assert!(
        run.alpha().contains("status = \"hidden\""),
        "needs 3 fresh good checks"
    );
    run.go(30, false).await;
    assert!(!run.alpha().contains("status"));
}

#[tokio::test]
async fn undecided_results_never_change_the_data() {
    let server = MockServer::start().await;
    let root = fixture("health-undecided", &server.uri());
    let run = Run {
        root: &root,
        server: &server,
    };
    let original = run.alpha();
    serve(&server, 403, 429, 503).await;
    for h in [0, 30, 60] {
        let r = run.go(h, false).await;
        assert!(!r.changed_data());
        assert_eq!(r.undecided.len(), 3);
    }
    assert_eq!(run.alpha(), original);
}
