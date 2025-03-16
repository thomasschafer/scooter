mod build_readme;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate README.md table of contents and config documentation
    Readme {
        /// Path to README.md file
        #[arg(long, default_value = "README.md")]
        readme: PathBuf,

        /// Path to config.rs file
        #[arg(long, default_value = "scooter/src/config.rs")]
        config: PathBuf,

        /// Only check if README is up to date, without modifying it
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Readme {
            readme,
            config,
            check,
        } => {
            build_readme::generate_readme(readme, config, *check)?;
        }
    }

    Ok(())
}
