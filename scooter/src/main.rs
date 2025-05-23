use clap::Parser;
use log::LevelFilter;
use logging::DEFAULT_LOG_LEVEL;
use std::str::FromStr;

use app_runner::{run_app, AppConfig};
use fields::{FieldValue, SearchFieldValues};

mod app;
mod app_runner;
mod config;
mod fields;
mod logging;
mod replace;
mod search;
mod tui;
mod ui;
mod utils;

#[derive(Parser, Debug)]
#[command(about = "Interactive find and replace TUI.")]
#[command(version)]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Directory in which to search
    #[arg(index = 1)]
    directory: Option<String>,

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

    // --- Initial values for fields ---
    //
    /// Text to search with
    #[arg(short = 's', long)]
    search_text: Option<String>,

    /// Text to replace the search text with
    #[arg(short = 'r', long)]
    replace_text: Option<String>,

    /// Search with plain strings, rather than regex
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    fixed_strings: bool,

    /// Only match when the search string forms an entire word, and not a substring in a larger word
    #[arg(short = 'w', long, action = clap::ArgAction::SetTrue)]
    match_whole_word: bool,

    /// Ignore case when matching the search string
    #[arg(short = 'i', long, action = clap::ArgAction::SetTrue)]
    case_insensitive: bool,

    /// Glob patterns, separated by commas (,), that file paths must match
    #[arg(short = 'I', long)]
    files_to_include: Option<String>,

    /// Glob patterns, separated by commas (,), that file paths must not match
    #[arg(short = 'E', long)]
    files_to_exclude: Option<String>,
}

fn parse_log_level(s: &str) -> Result<LevelFilter, String> {
    LevelFilter::from_str(s).map_err(|_| format!("Invalid log level: {s}"))
}

impl<'a> From<&'a Args> for AppConfig<'a> {
    fn from(args: &'a Args) -> Self {
        let mut search_field_values = SearchFieldValues::default();
        if let Some(ref search_text) = args.search_text {
            search_field_values.search = FieldValue::new(search_text, true);
        }
        if let Some(ref replace_text) = args.replace_text {
            search_field_values.replace = FieldValue::new(replace_text, true);
        }
        if args.fixed_strings {
            search_field_values.fixed_strings = FieldValue::new(args.fixed_strings, true);
        }
        if args.match_whole_word {
            search_field_values.match_whole_word = FieldValue::new(args.match_whole_word, true);
        }
        if args.case_insensitive {
            search_field_values.match_case = FieldValue::new(!args.case_insensitive, true);
        }
        if let Some(ref files_to_include) = args.files_to_include {
            search_field_values.include_files = FieldValue::new(files_to_include, true);
        }
        if let Some(ref files_to_exclude) = args.files_to_exclude {
            search_field_values.exclude_files = FieldValue::new(files_to_exclude, true);
        }

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
