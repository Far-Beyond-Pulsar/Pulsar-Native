//! `pulsar`: the Pulsar command-line tool.
//!
//! ```text
//! pulsar package --project <dir> --out <dir> [--profile dev|shipping] [--target <triple>]
//!                [--loose] [--skip-build] [--startup-level <path>] [--report <file>]
//! pulsar build-scripts --project <dir> [--profile dev|shipping]
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use pulsar_content::BuildProfile;

#[derive(Parser)]
#[command(name = "pulsar", version, about = "Pulsar engine command-line tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Package a project as a standalone game: compile scripts, cook
    /// levels and assets into Content/game.pak, build the executable.
    Package {
        /// The project directory.
        #[arg(long)]
        project: PathBuf,
        /// Output directory (gets the executable and Content/).
        #[arg(long)]
        out: PathBuf,
        /// Script limits and checks: `dev` or `shipping`.
        #[arg(long, default_value = "shipping")]
        profile: BuildProfile,
        /// Rust target triple of the game executable (default: host).
        #[arg(long)]
        target: Option<String>,
        /// Write Content/ as loose files instead of game.pak.
        #[arg(long)]
        loose: bool,
        /// Package content only; do not build the executable.
        #[arg(long)]
        skip_build: bool,
        /// Startup level, project-relative (default: Pulsar/project.json's,
        /// then the editor's default map).
        #[arg(long)]
        startup_level: Option<String>,
        /// Engine built-in assets directory (default: this engine's assets/).
        #[arg(long)]
        engine_assets: Option<PathBuf>,
        /// Also write the package report (JSON) here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Compile every script class of a project headlessly and check that
    /// each links against the engine. Exits non-zero on errors.
    BuildScripts {
        #[arg(long)]
        project: PathBuf,
        #[arg(long, default_value = "dev")]
        profile: BuildProfile,
    },
}

fn main() {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Package { project, out, profile, target, loose, skip_build, startup_level, engine_assets, report } => {
            let options = pulsar_package::PackageOptions {
                project,
                out,
                profile,
                target,
                loose,
                skip_build,
                startup_level,
                engine_assets,
            };
            match pulsar_package::package(&options) {
                Ok(result) => {
                    for warning in &result.warnings {
                        eprintln!("warning: {warning}");
                    }
                    let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                    if let Some(path) = report {
                        if let Err(error) = std::fs::write(&path, &json) {
                            eprintln!("error: cannot write {}: {error}", path.display());
                        }
                    }
                    println!(
                        "Packaged {} ({} profile): {} classes, {} levels, {} assets{}{}",
                        options.out.display(),
                        result.profile,
                        result.classes.len(),
                        result.levels.len(),
                        result.assets,
                        result.pak.as_ref().map(|p| format!(", {}", p.display())).unwrap_or_default(),
                        result
                            .executable
                            .as_ref()
                            .map(|p| format!(", {}", p.display()))
                            .unwrap_or_else(|| ", executable not built (--skip-build)".into()),
                    );
                    0
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    1
                }
            }
        }
        Command::BuildScripts { project, profile } => match pulsar_package::build_scripts(&project, profile) {
            Ok(output) => {
                for problem in &output.problems {
                    eprintln!("{problem}");
                }
                let scripted = output.classes.iter().filter(|c| c.module.is_some()).count();
                println!("{} classes, {scripted} with scripts: all compile and link", output.classes.len());
                0
            }
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        },
    };
    std::process::exit(code);
}
