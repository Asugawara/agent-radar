use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "radar",
    version,
    about = "agent-radar data tools and site builder"
)]
struct Cli {
    /// Repository root (contains data/, i18n/, site.toml)
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check data/tools, data/tags.toml and i18n; exit 1 on errors
    Validate,
    /// Rewrite tool files in canonical form
    Fmt {
        /// Only list files that would change (exit 1 if any)
        #[arg(long)]
        check: bool,
    },
    /// Write a draft data/tools/<id>.toml from a GitHub or web page URL
    Add {
        /// GitHub repository or product page
        url: String,
        /// File name / id (default: the name in kebab-case)
        #[arg(long)]
        id: Option<String>,
        /// Print the draft instead of writing it
        #[arg(long)]
        dry_run: bool,
    },
    /// Tag maintenance
    #[command(subcommand)]
    Tags(TagsCommand),
    /// Generate the static site
    Build {
        /// URL prefix for all links (default: base_url of site.toml)
        #[arg(long)]
        base_url: Option<String>,
        /// Output directory
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        /// JSON file {"owner/name": {stars, pushed_at, archived, license}}
        #[arg(long)]
        enrich: Option<PathBuf>,
        /// Directory made by `radar favicons`; its icons go into the site
        #[arg(long)]
        favicons: Option<PathBuf>,
    },
    /// Fetch a favicon per tool into a directory (with manifest.json)
    Favicons {
        /// Output directory (also the cache)
        #[arg(long)]
        out: PathBuf,
        /// Do not fetch again what is newer than this
        #[arg(long, default_value_t = 7)]
        max_age_days: u64,
    },
    /// Check links, hide dead tools, drop dead links (updates data/tools)
    CheckLinks {
        /// health.json holding the per-URL state (created if missing)
        #[arg(long)]
        state: PathBuf,
        /// Print what would change; write neither tools nor state
        #[arg(long)]
        dry_run: bool,
        /// Clock override (RFC 3339)
        #[arg(long, value_parser = radar_cli::health::run::parse_now)]
        now: Option<time::OffsetDateTime>,
    },
    /// Fetch stars / pushed_at / archived / license of all repos (needs GITHUB_TOKEN)
    Enrich {
        /// Output JSON file
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum TagsCommand {
    /// List tags per facet with usage counts
    List,
    /// Rename a tag everywhere
    Rename { old: String, new: String },
    /// Merge tag FROM into INTO everywhere
    Merge { from: String, into: String },
}

fn run(cli: Cli) -> anyhow::Result<bool> {
    use radar_cli::{build, commands};
    match cli.command {
        Command::Validate => commands::validate(&cli.root),
        Command::Fmt { check } => {
            let changed = commands::fmt(&cli.root, check)?;
            for p in &changed {
                println!("{p}");
            }
            if check && !changed.is_empty() {
                eprintln!("{} file(s) not formatted; run `radar fmt`", changed.len());
            }
            Ok(check && !changed.is_empty())
        }
        Command::Add { url, id, dry_run } => {
            radar_cli::add::run_cli(&cli.root, &url, id.as_deref(), dry_run).map(|_| false)
        }
        Command::Tags(TagsCommand::List) => commands::tags_list(&cli.root).map(|_| false),
        Command::Tags(TagsCommand::Rename { old, new }) => {
            let n = commands::tags_change(&cli.root, &old, &new, false)?;
            println!("renamed \"{old}\" -> \"{new}\" in {} tool file(s)", n.len());
            Ok(false)
        }
        Command::Tags(TagsCommand::Merge { from, into }) => {
            let n = commands::tags_change(&cli.root, &from, &into, true)?;
            println!(
                "merged \"{from}\" into \"{into}\" in {} tool file(s)",
                n.len()
            );
            Ok(false)
        }
        Command::Build {
            base_url,
            out,
            enrich,
            favicons,
        } => build::run(
            &cli.root,
            &out,
            base_url.as_deref(),
            enrich.as_deref(),
            favicons.as_deref(),
        ),
        Command::Favicons { out, max_age_days } => {
            radar_cli::favicons::run_cli(&cli.root, &out, max_age_days).map(|_| false)
        }
        Command::CheckLinks {
            state,
            dry_run,
            now,
        } => radar_cli::health::run_cli(&cli.root, &state, dry_run, now).map(|_| false),
        Command::Enrich { out } => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(radar_cli::github::enrich(&cli.root, &out))
                .map(|_| false)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
