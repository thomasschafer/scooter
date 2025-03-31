use crate::app::SearchFieldValues;
use app_runner::{run_app, AppConfig};
use clap::Parser;
use log::LevelFilter;
use logging::DEFAULT_LOG_LEVEL;
use std::str::FromStr;

mod app;
mod app_runner;
mod config;
mod fields;
mod logging;
mod replace;
mod tui;
mod ui;
mod utils;

#[derive(Parser, Debug)]
#[command(about = "Interactive find and replace TUI.")]
#[command(version)]
struct Args {
    /// Directory in which to search
    #[arg(index = 1)]
    directory: Option<String>,

    #[arg(long, default_value = None)]
    search_text: Option<String>,

    /// Include hidden files and directories, such as those whose name starts with a dot (.)
    #[arg(short = '.', long, default_value = "false")]
    hidden: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(
        long,
        value_parser = parse_log_level,
        default_value = DEFAULT_LOG_LEVEL
    )]
    log_level: LevelFilter,

    /// Use advanced regex features (including negative look-ahead), at the cost of performance
    #[arg(short = 'a', long, default_value = "false")]
    advanced_regex: bool,
}

fn parse_log_level(s: &str) -> Result<LevelFilter, String> {
    LevelFilter::from_str(s).map_err(|_| format!("Invalid log level: {}", s))
}

impl<'a> From<&'a Args> for AppConfig<'a> {
    fn from(args: &'a Args) -> Self {
        let mut search_field_values = SearchFieldValues::default();
        if let Some(ref search_text) = args.search_text {
            search_field_values.search = search_text;
        };

        Self {
            directory: args.directory.clone(),
            hidden: args.hidden,
            advanced_regex: args.advanced_regex,
            log_level: args.log_level,
            search_field_values,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = AppConfig::from(&args);
    run_app(config).await
}
