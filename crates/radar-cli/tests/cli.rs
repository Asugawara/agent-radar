use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

/// A private copy of the `basic` fixture.
fn fixture(name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    dir
}

fn radar(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radar"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn edit(path: &Path, f: impl Fn(&str) -> String) {
    let s = fs::read_to_string(path).unwrap();
    fs::write(path, f(&s)).unwrap();
}

#[test]
fn valid_fixture_passes() {
    let root = fixture("valid");
    let o = radar(&root, &["validate"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(radar(&root, &["fmt", "--check"]).status.success());
}

#[test]
fn unknown_tag_fails_with_suggestion() {
    let root = fixture("unknown-tag");
    edit(&root.join("data/tools/beta.toml"), |s| {
        s.replace("\"cli\"", "\"clii\"")
    });
    let o = radar(&root, &["validate"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stdout(&o).contains("data/tools/beta.toml: unknown tag \"clii\" (did you mean \"cli\"?)"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn category_count_is_enforced() {
    let root = fixture("category-count");
    let beta = root.join("data/tools/beta.toml");
    edit(&beta, |s| s.replace("\"memory\", \"gateway\", ", ""));
    let o = radar(&root, &["validate"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stdout(&o).contains("facet \"category\" needs at least 1 tag(s), found 0"),
        "{}",
        stdout(&o)
    );

    edit(&beta, |s| {
        s.replace(
            "tags = [",
            "tags = [\"coding-agent\", \"sandbox\", \"memory\", \"gateway\", ",
        )
    });
    let o = radar(&root, &["validate"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stdout(&o).contains("facet \"category\" allows at most 3 tag(s), found 4"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn missing_summary_ja_fails() {
    let root = fixture("summary-ja");
    edit(&root.join("data/tools/alpha.toml"), |s| {
        s.replace("summary.ja = \"Alpha の日本語説明\"\n", "")
    });
    let o = radar(&root, &["validate"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stdout(&o).contains("summary.ja is missing"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn i18n_mismatch_fails() {
    let root = fixture("i18n");
    edit(&root.join("i18n/ja.toml"), |s| {
        s.replace("home = \"ホーム\"\n", "")
    });
    let o = radar(&root, &["validate"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stdout(&o).contains("i18n/ja.toml: key \"header.home\" is missing"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn fmt_check_then_fmt() {
    let root = fixture("fmt");
    let alpha = root.join("data/tools/alpha.toml");
    edit(&alpha, |s| {
        format!(
            "# keep me\n{}",
            s.replace("name = \"Alpha\"\n", "")
                .replace("tags =", "name = \"Alpha\"\ntags =")
        )
    });
    let o = radar(&root, &["fmt", "--check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("data/tools/alpha.toml"));
    assert!(radar(&root, &["fmt"]).status.success());
    assert!(radar(&root, &["fmt", "--check"]).status.success());
    let text = fs::read_to_string(&alpha).unwrap();
    assert!(text.contains("# keep me"));
    assert!(text.find("name =").unwrap() < text.find("url =").unwrap());
}

#[test]
fn tags_rename_and_merge() {
    let root = fixture("tags");
    let o = radar(&root, &["tags", "rename", "parallel", "concurrent"]);
    assert!(o.status.success(), "{:?}", o);
    assert!(
        fs::read_to_string(root.join("data/tools/alpha.toml"))
            .unwrap()
            .contains("\"concurrent\"")
    );
    assert!(
        fs::read_to_string(root.join("data/tags.toml"))
            .unwrap()
            .contains("aliases = [\"parallel\"]")
    );
    assert!(radar(&root, &["validate"]).status.success());
    assert!(radar(&root, &["fmt", "--check"]).status.success());

    let o = radar(&root, &["tags", "merge", "gateway", "memory"]);
    assert!(o.status.success(), "{:?}", o);
    let beta = fs::read_to_string(root.join("data/tools/beta.toml")).unwrap();
    assert_eq!(beta.matches("\"memory\"").count(), 1);
    assert!(!beta.contains("gateway"));
    assert!(radar(&root, &["validate"]).status.success());
    // Using the old name now points at the new one.
    edit(&root.join("data/tools/beta.toml"), |s| {
        s.replace("\"memory\"", "\"gateway\"")
    });
    let o = radar(&root, &["validate"]);
    assert!(
        stdout(&o).contains("tag \"gateway\" is an alias; use \"memory\""),
        "{}",
        stdout(&o)
    );
}

#[test]
fn build_renders_both_languages() {
    let root = fixture("build");
    let out = root.join("out");
    let o = radar(
        &root,
        &["build", "--base-url", "/x/", "--out", out.to_str().unwrap()],
    );
    assert!(o.status.success(), "{:?}", o);
    for p in [
        "index.html",
        "ja/index.html",
        "ja/removed/index.html",
        "tools.json",
        "static/style.css",
        "static/brand/favicon-32.png",
        "static/brand/apple-touch-icon.png",
        "static/brand/logo-48.png",
    ] {
        assert!(out.join(p).exists(), "missing {p}");
    }
    let en = fs::read_to_string(out.join("index.html")).unwrap();
    let ja = fs::read_to_string(out.join("ja/index.html")).unwrap();
    assert!(en.contains("href=\"/x/static/brand/favicon-32.png\""));
    assert!(en.contains("src=\"/x/static/brand/logo-48.png\""));
    assert_eq!(en.matches("<tr").count(), 3);
    assert!(
        en.contains("<div id=\"filters\">")
            && en.contains("id=\"site-data\"")
            && !en.contains("class=\"toolbar\"")
            && en.contains("class=\"badge badge-category\"")
            && en.contains("<span class=\"ic\"><svg class=\"icon\"")
    );
    assert!(
        en.contains("hreflang=\"ja\" href=\"https://asugawara.github.io/x/ja/\"")
            && ja.contains("hreflang=\"en\" href=\"https://asugawara.github.io/x/\"")
            && en.contains("rel=\"canonical\" href=\"https://asugawara.github.io/x/\"")
            && ja.contains("rel=\"canonical\" href=\"https://asugawara.github.io/x/ja/\"")
    );
    assert!(ja.contains("Alpha の日本語説明") && ja.contains("コーディングエージェント"));
    assert!(
        en.contains(">—<"),
        "stars should be a dash without enrich data"
    );
    for (html, base) in [(&en, "/x/ja/"), (&ja, "/x/")] {
        assert!(html.contains(&format!("data-base=\"{base}\"")));
        assert!(html.contains("a.href = a.dataset.base + location.search + location.hash;"));
        assert!(html.contains("localStorage.setItem('lang', a.hreflang)"));
    }
    // Only the list pages are built: no per-tool or per-tag pages, no third-party scripts.
    assert!(!en.contains("googletagmanager"));
    for gone in ["tools", "tags", "ja/tools", "ja/tags"] {
        assert!(!out.join(gone).exists(), "{gone} should not be built");
    }
    assert!(
        !en.contains("script src=\"https://") && !en.contains("/tools/") && !en.contains("/tags/")
    );
    // Name -> official site; category badge -> filter link in the page language.
    assert!(en.contains("<a class=\"tool-name\" href=\"https://alpha.example\" target=\"_blank\" rel=\"noopener\">Alpha</a>"), "{en}");
    assert!(
        en.contains("class=\"badge badge-category\" href=\"/x/?tag=coding-agent\""),
        "{en}"
    );
    assert!(
        ja.contains("class=\"badge badge-category\" href=\"/x/ja/?tag=coding-agent\""),
        "{ja}"
    );
    for f in ["index.html", "ja/index.html"] {
        assert!(
            !fs::read_to_string(out.join(f)).unwrap().contains("[["),
            "{f} has a missing i18n key"
        );
    }
}

#[test]
fn build_embeds_gtm_when_configured() {
    let root = fixture("gtm");
    edit(&root.join("site.toml"), |s| {
        format!("{s}gtm_id = \"GTM-TEST123\"\n")
    });
    let out = root.join("out");
    let o = radar(
        &root,
        &["build", "--base-url", "/x/", "--out", out.to_str().unwrap()],
    );
    assert!(o.status.success(), "{:?}", o);
    for p in ["index.html", "ja/index.html"] {
        let html = fs::read_to_string(out.join(p)).unwrap();
        assert!(html.contains("gtm.js?id=GTM-TEST123"), "{p}");
        assert!(html.contains("gtag('js', new Date())"), "{p}");
        assert!(
            html.contains("location.origin === \"https://asugawara.github.io\""),
            "{p}"
        );
        assert!(html.contains("!window.radarLangRedirect"), "{p}");
    }
}

#[test]
fn build_redirects_saved_language_before_gtm() {
    let root = fixture("lang-redirect");
    edit(&root.join("site.toml"), |s| {
        format!("{s}gtm_id = \"GTM-TEST123\"\n")
    });
    let out = root.join("out");
    let o = radar(
        &root,
        &["build", "--base-url", "/x/", "--out", out.to_str().unwrap()],
    );
    assert!(o.status.success(), "{:?}", o);
    let cases = [
        ("index.html", "\"/x/ja/\"", "ja"),
        ("ja/index.html", "\"/x/\"", "en"),
    ];
    for (p, target, lang) in cases {
        let html = fs::read_to_string(out.join(p)).unwrap();
        let check = html
            .find(&format!("l === \"{lang}\""))
            .unwrap_or_else(|| panic!("{p}: no saved-language check"));
        assert!(html.contains("localStorage.getItem('lang')"), "{p}");
        assert!(
            html.contains(target),
            "{p}: missing redirect target {target}"
        );
        let gtm = html.find("gtm.js?id=").expect("gtm script");
        assert!(check < gtm, "{p}: redirect must come before GTM");
    }
}

#[test]
fn build_rejects_malformed_gtm_id() {
    let root = fixture("gtm-bad");
    edit(&root.join("site.toml"), |s| {
        format!("{s}gtm_id = \"GTM-1'</script>\"\n")
    });
    let out = root.join("out");
    let o = radar(
        &root,
        &["build", "--base-url", "/x/", "--out", out.to_str().unwrap()],
    );
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("gtm_id"),
        "{:?}",
        o
    );
}

#[test]
fn build_embeds_table_ui_and_copies_wasm_when_present() {
    let root = fixture("wasm");
    let out = root.join("out");
    let build = |root: &Path| {
        let o = radar(
            root,
            &["build", "--base-url", "/x/", "--out", out.to_str().unwrap()],
        );
        assert!(o.status.success(), "{:?}", o);
    };
    build(&root);
    let en = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(en.contains("id=\"ui-strings\"") && en.contains("data-base=\"/x/\""));
    assert!(en.contains("{shown} / {total}"));
    assert!(!en.contains("type=\"module\"") && !out.join("pkg").exists());
    let ja = fs::read_to_string(out.join("ja/index.html")).unwrap();
    assert!(ja.contains("data-base=\"/x/ja/\"") && ja.contains("絞り込み"));

    let pkg = root.join("crates/radar-web/pkg");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(pkg.join("radar_web.js"), "export default function(){}").unwrap();
    fs::write(pkg.join("radar_web_bg.wasm"), "\0asm").unwrap();
    build(&root);
    let en = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(en.contains("<script type=\"module\">import init from \"/x/pkg/radar_web.js\""));
    assert!(out.join("pkg/radar_web_bg.wasm").exists());
    assert!(
        en.contains("class=\"toolbar\"")
            && en.contains("aria-expanded=\"false\"")
            && en.contains("<span class=\"result-count\">2 / 2</span>")
    );
}

#[test]
fn build_with_enrich_and_hidden() {
    let root = fixture("enrich");
    edit(&root.join("data/tools/beta.toml"), |s| {
        format!(
            "{s}repo = \"o/beta\"\nstatus = \"hidden\"\nhidden_reason = \"url: 404\"\nhidden_since = 2026-11-05\n"
        )
    });
    edit(&root.join("data/tools/alpha.toml"), |s| {
        format!("{s}repo = \"o/alpha\"\n")
    });
    fs::write(
        root.join("enrich.json"),
        r#"{"o/alpha": {"stars": 1234, "pushed_at": "2026-11-01T00:00:00Z", "archived": false, "license": "MIT"}}"#,
    )
    .unwrap();
    let out = root.join("out");
    let o = radar(
        &root,
        &[
            "build",
            "--out",
            out.to_str().unwrap(),
            "--enrich",
            root.join("enrich.json").to_str().unwrap(),
        ],
    );
    assert!(
        o.status.success(),
        "{} {}",
        stdout(&o),
        String::from_utf8_lossy(&o.stderr)
    );
    let en = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(en.contains("1.2k</td>") && en.contains("title=\"1234\"") && !en.contains("Beta"));
    assert!(en.contains("title=\"2026-11-01\">") && en.contains(" ago</td>"));
    // alpha has a repository: a small GitHub link next to the name.
    assert!(
        en.contains("class=\"repo-link\" href=\"https://github.com/o/alpha\" target=\"_blank\" rel=\"noopener\" title=\"GitHub\" aria-label=\"GitHub\""),
        "{en}"
    );
    let removed = fs::read_to_string(out.join("removed/index.html")).unwrap();
    assert!(
        removed.contains("Beta") && removed.contains("url: 404") && removed.contains("2026-11-05")
    );
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("tools.json")).unwrap()).unwrap();
    assert_eq!(json["tools"][0]["stars"], 1234);
    assert_eq!(json["tools"].as_array().unwrap().len(), 1);
}

#[test]
fn build_with_favicons_shows_image_or_initial() {
    let root = fixture("favicons");
    let icons = root.join("icons");
    fs::create_dir_all(&icons).unwrap();
    fs::write(icons.join("alpha.png"), [0x89, b'P', b'N', b'G']).unwrap();
    fs::write(
        icons.join("manifest.json"),
        r#"{"alpha":{"file":"alpha.png","source":"https://a.example/i.png","kind":"html","fetched_at":"2026-11-02T00:00:00Z","error":null},
            "beta":{"file":null,"source":null,"kind":null,"fetched_at":"2026-11-02T00:00:00Z","error":"dns"}}"#,
    )
    .unwrap();
    let out = root.join("out");
    let o = radar(
        &root,
        &[
            "build",
            "--base-url",
            "/x/",
            "--out",
            out.to_str().unwrap(),
            "--favicons",
            icons.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{:?}", o);
    assert!(out.join("favicons/alpha.png").is_file());
    assert!(!out.join("favicons/beta.png").exists());
    let en = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(en.contains(
        "<img src=\"/x/favicons/alpha.png\" width=\"32\" height=\"32\" loading=\"lazy\" alt=\"\""
    ));
    // beta has no icon: its initial in a circle, and no favicon <img>.
    // (The header logo is an <img> too, so only count favicon images.)
    assert_eq!(en.matches("<img src=\"/x/favicons/").count(), 1);
    assert!(en.contains("class=\"fav-i\"") && en.contains(">B</span>"));
    assert!(en.contains("\"favicon\":\"/x/favicons/alpha.png\""));
}

#[test]
fn build_shows_works_with_and_platform_columns() {
    let root = fixture("works_with");
    edit(&root.join("data/tags.toml"), |s| {
        s.to_owned()
            + "\n[[facet]]\n\
             id = \"works-with\"\n\
             label = { en = \"Works with\", ja = \"対応エージェント\" }\n\
             min = 0\n\n\
             [[facet.tag]]\n\
             id = \"works-with-claude-code\"\n\
             label = { en = \"Claude Code\", ja = \"Claude Code\" }\n\
             description = { en = \"Claude Code d\", ja = \"Claude Code 説明\" }\n"
    });
    edit(&root.join("data/tools/alpha.toml"), |s| {
        s.replace(
            "tags = [\"coding-agent\", \"sandbox\", \"cli\", \"parallel\"]",
            "tags = [\"coding-agent\", \"sandbox\", \"cli\", \"parallel\", \"works-with-claude-code\"]",
        )
        .replace("platforms = [\"linux\"]", "platforms = [\"linux\", \"macos\"]")
    });
    edit(&root.join("data/tools/beta.toml"), |s| {
        s.replace("platforms = [\"linux\"]", "platforms = [\"unspecified\"]")
    });
    let out = root.join("out");
    let o = radar(
        &root,
        &["build", "--base-url", "/x/", "--out", out.to_str().unwrap()],
    );
    assert!(o.status.success(), "{:?}", o);

    // The table row of one tool, from its `<tr>` to its `</tr>`.
    let row = |html: &str, id: &str| {
        let start = html.find(&format!("<tr data-id=\"{id}\">")).unwrap();
        let end = start + html[start..].find("</tr>").unwrap();
        html[start..end].to_string()
    };
    let en = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(en.contains("<th class=\"col-works\">"));
    assert!(en.contains("<th class=\"col-platforms\">"));
    let alpha = row(&en, "alpha");
    assert!(alpha.contains("href=\"/x/?tag=works-with-claude-code\""));
    let mac = alpha.find("href=\"/x/?platform=macos\"").unwrap();
    let linux = alpha.find("href=\"/x/?platform=linux\"").unwrap();
    assert!(mac < linux, "macOS should come before Linux: {alpha}");
    let beta = row(&en, "beta");
    assert!(beta.contains("<td class=\"col-platforms muted\">Not stated</td>"));
    assert!(beta.contains("<td class=\"col-works muted\">"));

    let ja = fs::read_to_string(out.join("ja/index.html")).unwrap();
    assert!(ja.contains("対応エージェント"));
    assert!(ja.contains("言及なし"));
}

#[test]
fn issue_form_categories_are_checked_against_tags() {
    let root = fixture("issue_form");
    // No form in the fixture: nothing to check.
    assert!(radar(&root, &["validate"]).status.success());
    let form = root.join(".github/ISSUE_TEMPLATE/suggest-tool.yml");
    fs::create_dir_all(form.parent().unwrap()).unwrap();
    let write = |opts: &str| {
        fs::write(
            &form,
            format!("body:\n  - type: dropdown\n    id: category\n    attributes:\n      options:\n{opts}"),
        )
        .unwrap()
    };
    write(
        "        - Coding agent\n        - Sandbox\n        - Memory\n        - Gateway\n        - Not sure / わからない\n",
    );
    assert!(radar(&root, &["validate"]).status.success());
    write("        - Coding agent\n        - Not sure / わからない\n");
    let o = radar(&root, &["validate"]);
    assert!(!o.status.success());
    let out = stdout(&o);
    assert!(out.contains("suggest-tool.yml"), "{out}");
    assert!(out.contains("        - Gateway"), "{out}");
}
