# agent-radar

A self-maintaining directory of tools for AI agents, built in Rust and published on GitHub Pages.

**Site: <https://asugawara.github.io/agent-radar/>** (日本語: <https://asugawara.github.io/agent-radar/ja/>)

- Filter and sort the table by tags, pricing, license and platform
- Dead links are checked every 6 hours and removed automatically
- English (`/`) and Japanese (`/ja/`)

エージェント向けツールの一覧サイトです。リンク切れは自動で検査・削除されます。

## Add a tool

Three ways, from most to least hands-on:

1. **Draft with the CLI.** `cargo run -p radar-cli -- add <URL>` (a GitHub repository or a product page; add `--id <id>` to choose the file name, `--dry-run` to only print) writes `data/tools/<id>.toml` with name, URL, repo, English summary and license filled in. Fill in `summary.ja`, `platforms`, `tags` and check `pricing`, run `cargo run -p radar-cli -- validate`, then open a pull request. A fresh draft fails `validate` until those are filled in.
2. **Create the file on GitHub.** Use "Add a tool" in the site footer: it opens GitHub's new-file screen with a template (rename the file to the tool's id).
3. **Just suggest it.** Use "Suggest a tool" in the footer (or [open an issue](https://github.com/Asugawara/agent-radar/issues/new?template=suggest-tool.yml)) and a maintainer adds it.

After a merge, stars, last update and the favicon are filled in automatically.

## Development

Requirements: Rust stable with the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and [pre-commit](https://pre-commit.com/). Optional: `gitleaks` and `shellcheck` (the hooks download their own copies; install them only to run the same checks by hand).

```sh
pre-commit install            # installs the pre-commit and pre-push hooks
bash scripts/build-site.sh dist --base-url /
python3 -m http.server 8765 -d dist   # open http://localhost:8765/
```

`build-site.sh` fetches stars and favicons only when `GITHUB_TOKEN` is set / `RADAR_SKIP_FAVICONS` is not `1`; wasm-bindgen-cli is installed under `target/tools` when missing.

When the hooks run:

| Stage | Hook | Runs when |
| --- | --- | --- |
| commit | whitespace, EOF, YAML/TOML, merge-conflict, large-file, line-ending checks | every commit |
| commit | gitleaks, shellcheck | every commit (shellcheck: `*.sh` only) |
| commit | `cargo fmt --check` | `*.rs` changed |
| commit | `radar validate`, `radar fmt --check` | `data/`, `i18n/`, `site.toml` or `.github/ISSUE_TEMPLATE/` changed |
| push | `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` | `*.rs` changed |

Run everything by hand with `pre-commit run --all-files` (add `--hook-stage pre-push` for the slow ones). CI (`.github/workflows/ci.yml`) runs the same checks plus a full site build.

## Deploy

1. Settings → Pages → Source: **GitHub Actions**.
2. `deploy.yml` publishes on every push to `main`. `maintain.yml` runs every 6 hours: it checks links, commits data changes to `main`, stores link health on the `bot-state` branch and then calls `deploy.yml`.

## License

MIT (code and data). See [LICENSE](LICENSE).

Icons: Lucide (ISC). See [static/icons/LICENSE](static/icons/LICENSE).
