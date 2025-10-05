use anyhow::bail;
use clap::Parser;
use frep_core::validation::{DirConfig, SearchConfig};
use log::LevelFilter;
use std::{
    io::{self, IsTerminal, Read},
    path::PathBuf,
    str::FromStr,
};

use scooter_core::{
    app::AppRunConfig,
    fields::{FieldValue, SearchFieldValues},
};

use app_runner::{run_app_tui, AppConfig};
use headless::{run_headless, run_headless_with_stdin};
use logging::{setup_logging, DEFAULT_LOG_LEVEL};

mod app_runner;
mod config;
mod conversions;
mod headless;
mod logging;
mod tui;
mod ui;

#[derive(Parser, Debug)]
#[command(about = "Interactive find and replace TUI.")]
#[command(version)]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Directory in which to search
    #[arg(index = 1, value_parser = parse_directory, default_value = ".")]
    directory: PathBuf,

    /// Include hidden files and directories, such as those whose name starts with a dot (.)
    #[arg(short = '.', long, action = clap::ArgAction::SetTrue)]
    hidden: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(
        long,
        value_parser = parse_log_level,
        default_value = DEFAULT_LOG_LEVEL
    )]
    log_level: LevelFilter,

    /// Use advanced regex features (including negative look-ahead), at the cost of performance
    #[arg(short = 'a', long, action = clap::ArgAction::SetTrue)]
    advanced_regex: bool,

    /// Search immediately using values set by flags (e.g. `--search_text`), rather than showing search fields first
    #[arg(short = 'S', long)]
    immediate_search: bool,

    /// Replace immediately once search completes, without waiting for confirmation
    #[arg(short = 'R', long)]
    immediate_replace: bool,

    /// Print results to stdout, rather than displaying them as a final screen
    #[arg(short = 'P', long)]
    print_results: bool,

    /// Combines `immediate_search`, `immediate_replace` and `print_results`
    #[arg(short = 'X', long)]
    immediate: bool,

    /// Run scooter without a TUI. Search and replace runs immediately (as with the `--immediate` flag), but with no user interface
    #[arg(short = 'N', long)]
    no_tui: bool,

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

    /// Override the config directory (default: ~/.config/scooter on Linux/macOS, %AppData%\scooter on Windows)
    #[arg(long)]
    config_dir: Option<PathBuf>,
}

fn parse_log_level(s: &str) -> Result<LevelFilter, String> {
    LevelFilter::from_str(s).map_err(|_| format!("Invalid log level: {s}"))
}

fn parse_directory(dir: &str) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(dir);
    if path.exists() {
        Ok(path)
    } else {
        bail!("'{dir}' does not exist. Please provide a valid path.")
    }
}

fn validate_flag_combinations(args: &Args) -> anyhow::Result<()> {
    if args.no_tui && args.immediate {
        bail!("--no-tui cannot be combined with --immediate");
    }

    if args.immediate_search || args.immediate_replace || args.print_results {
        for (name, enabled) in [("--no-tui", args.no_tui), ("--immediate", args.immediate)] {
            if enabled {
                bail!("{name} cannot be combined with --immediate-search, --immediate-replace, or --print-results");
            }
        }
    }

    Ok(())
}

fn validate_search_text_required(args: &Args) -> anyhow::Result<()> {
    if args.search_text.as_ref().is_none_or(String::is_empty) {
        for (name, enabled) in [
            ("--immediate-search", args.immediate_search),
            ("--immediate", args.immediate),
            ("--no-tui", args.no_tui),
        ] {
            if enabled {
                bail!("{name} requires --search-text to be provided");
            }
        }
    }

    Ok(())
}

fn detect_and_read_stdin() -> anyhow::Result<Option<String>> {
    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut stdin_content = String::new();
    io::stdin().read_to_string(&mut stdin_content)?;

    Ok(Some(stdin_content))
}

fn validate_stdin_usage(args: &Args, stdin_content: Option<&str>) -> anyhow::Result<()> {
    if stdin_content.is_some() {
        // File system args
        if args.hidden {
            bail!("Cannot use --hidden flag when processing stdin");
        }
        if args.files_to_include.is_some() {
            bail!("Cannot use --files-to-include when processing stdin");
        }
        if args.files_to_exclude.is_some() {
            bail!("Cannot use --files-to-exclude when processing stdin");
        }
    }
    Ok(())
}

impl<'a> TryFrom<&'a Args> for AppConfig<'a> {
    type Error = anyhow::Error;

    fn try_from(args: &'a Args) -> anyhow::Result<Self> {
        let stdin_content = detect_and_read_stdin()?;

        validate_flag_combinations(args)?;
        validate_search_text_required(args)?;
        validate_stdin_usage(args, stdin_content.as_deref())?;

        let immediate = args.immediate || args.no_tui;

        Ok(Self {
            directory: args.directory.clone(),
            log_level: args.log_level,
            search_field_values: args.into(),
            app_run_config: AppRunConfig {
                include_hidden: args.hidden,
                advanced_regex: args.advanced_regex,
                immediate_search: args.immediate_search || immediate,
                immediate_replace: args.immediate_replace || immediate,
                print_results: args.print_results || immediate,
            },
            stdin_content,
        })
    }
}

impl<'a> From<&'a Args> for SearchFieldValues<'a> {
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

        search_field_values
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    config::set_config_dir_override(args.config_dir.clone());
    let config = AppConfig::try_from(&args)?;
    setup_logging(config.log_level)?;

    let results = if args.no_tui {
        let results = if let Some(stdin_content) = config.stdin_content {
            run_headless_with_stdin(&stdin_content, search_config_from_args(&args))?
        } else {
            run_headless(search_config_from_args(&args), dir_config_from_args(&args))?
        };
        Some(results)
    } else {
        run_app_tui(config).await?
    };

    if let Some(results) = results {
        print!("{results}");
    }

    Ok(())
}

fn dir_config_from_args(args: &Args) -> DirConfig<'_> {
    DirConfig {
        include_globs: args.files_to_include.as_deref(),
        exclude_globs: args.files_to_exclude.as_deref(),
        include_hidden: args.hidden,
        directory: args.directory.clone(),
    }
}

fn search_config_from_args(args: &Args) -> SearchConfig<'_> {
    SearchConfig {
        search_text: args.search_text.as_deref().unwrap_or(""),
        replacement_text: args.replace_text.as_deref().unwrap_or(""),
        fixed_strings: args.fixed_strings,
        advanced_regex: args.advanced_regex,
        match_whole_word: args.match_whole_word,
        match_case: !args.case_insensitive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::LevelFilter;
    use std::env;
    use tempfile::TempDir;

    fn default_args() -> Args {
        Args {
            directory: env::current_dir().unwrap(),
            hidden: false,
            log_level: LevelFilter::Info,
            advanced_regex: false,
            immediate_search: false,
            immediate_replace: false,
            print_results: false,
            immediate: false,
            no_tui: false,
            search_text: None,
            replace_text: None,
            fixed_strings: false,
            match_whole_word: false,
            case_insensitive: false,
            files_to_include: None,
            files_to_exclude: None,
            config_dir: None,
        }
    }

    #[test]
    fn test_validate_flag_combinations_success() {
        let args = default_args();
        assert!(validate_flag_combinations(&args).is_ok());
    }

    #[test]
    fn test_validate_flag_combinations_no_tui_and_immediate() {
        let args = Args {
            no_tui: true,
            immediate: true,
            ..default_args()
        };
        let result = validate_flag_combinations(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("--no-tui cannot be combined with --immediate"));
    }

    #[test]
    fn test_validate_flag_combinations_no_tui_with_individual_flags() {
        let test_cases = [
            (
                "--immediate-search",
                Args {
                    no_tui: true,
                    immediate_search: true,
                    ..default_args()
                },
            ),
            (
                "--immediate-replace",
                Args {
                    no_tui: true,
                    immediate_replace: true,
                    ..default_args()
                },
            ),
            (
                "--print-results",
                Args {
                    no_tui: true,
                    print_results: true,
                    ..default_args()
                },
            ),
        ];

        for (flag_name, args) in test_cases {
            let result = validate_flag_combinations(&args);
            assert!(
                result.is_err(),
                "Expected error for --no-tui with {flag_name}"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("--no-tui cannot be combined with"));
        }
    }

    #[test]
    fn test_validate_flag_combinations_immediate_with_individual_flags() {
        let test_cases = [
            (
                "--immediate-search",
                Args {
                    immediate: true,
                    immediate_search: true,
                    ..default_args()
                },
            ),
            (
                "--immediate-replace",
                Args {
                    immediate: true,
                    immediate_replace: true,
                    ..default_args()
                },
            ),
            (
                "--print-results",
                Args {
                    immediate: true,
                    print_results: true,
                    ..default_args()
                },
            ),
        ];

        for (flag_name, args) in test_cases {
            let result = validate_flag_combinations(&args);
            assert!(
                result.is_err(),
                "Expected error for --immediate with {flag_name}"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("--immediate cannot be combined with"));
        }
    }

    #[test]
    fn test_validate_search_text_required_success() {
        let args = Args {
            search_text: Some("test".to_string()),
            immediate: true,
            ..default_args()
        };
        assert!(validate_search_text_required(&args).is_ok());
    }

    #[test]
    fn test_validate_search_text_required_flags_without_text() {
        let test_cases = [
            (
                "--immediate-search",
                Args {
                    immediate_search: true,
                    ..default_args()
                },
            ),
            (
                "--immediate",
                Args {
                    immediate: true,
                    ..default_args()
                },
            ),
            (
                "--no-tui",
                Args {
                    no_tui: true,
                    ..default_args()
                },
            ),
        ];

        for (flag_name, args) in test_cases {
            let result = validate_search_text_required(&args);
            assert!(
                result.is_err(),
                "Expected error for {flag_name} without search text"
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains(&format!("{flag_name} requires --search-text")));
        }
    }

    #[test]
    fn test_app_config_try_from_success() {
        let args = Args {
            directory: PathBuf::from("/test"),
            search_text: Some("test".to_string()),
            immediate: true,
            ..default_args()
        };
        let config = AppConfig::try_from(&args);
        assert!(config.is_ok());

        let config = config.unwrap();
        assert_eq!(config.directory, PathBuf::from("/test"));
        assert!(config.app_run_config.immediate_search);
        assert!(config.app_run_config.immediate_replace);
        assert!(config.app_run_config.print_results);
    }

    #[test]
    fn test_search_field_values_from() {
        let args = Args {
            search_text: Some("test_search".to_string()),
            replace_text: Some("test_replace".to_string()),
            fixed_strings: true,
            match_whole_word: true,
            case_insensitive: true,
            files_to_include: Some("*.rs".to_string()),
            files_to_exclude: Some("target/*".to_string()),
            ..default_args()
        };

        let values = SearchFieldValues::from(&args);

        assert_eq!(values.search.value, "test_search");
        assert_eq!(values.search.set_by_cli, true);

        assert_eq!(values.replace.value, "test_replace");
        assert_eq!(values.replace.set_by_cli, true);

        assert_eq!(values.fixed_strings.value, true);
        assert_eq!(values.fixed_strings.set_by_cli, true);

        assert_eq!(values.match_whole_word.value, true);
        assert_eq!(values.match_whole_word.set_by_cli, true);

        assert_eq!(values.match_case.value, false);
        assert_eq!(values.match_case.set_by_cli, true);

        assert_eq!(values.include_files.value, "*.rs");
        assert_eq!(values.include_files.set_by_cli, true);

        assert_eq!(values.exclude_files.value, "target/*");
        assert_eq!(values.exclude_files.set_by_cli, true);
    }

    #[test]
    fn test_search_field_values_from_defaults() {
        let args = default_args();
        let values = SearchFieldValues::from(&args);

        assert_eq!(values.search.value, "");
        assert_eq!(values.search.set_by_cli, false);

        assert_eq!(values.replace.value, "");
        assert_eq!(values.replace.set_by_cli, false);

        assert_eq!(values.fixed_strings.value, false);
        assert_eq!(values.fixed_strings.set_by_cli, false);

        assert_eq!(values.match_whole_word.value, false);
        assert_eq!(values.match_whole_word.set_by_cli, false);

        assert_eq!(values.match_case.value, true);
        assert_eq!(values.match_case.set_by_cli, false);

        assert_eq!(values.include_files.value, "");
        assert_eq!(values.include_files.set_by_cli, false);

        assert_eq!(values.exclude_files.value, "");
        assert_eq!(values.exclude_files.set_by_cli, false);
    }

    fn setup_test_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_validate_directory_exists() {
        let temp_dir = setup_test_dir();
        let dir_path = temp_dir.path().to_str().unwrap();

        let result = parse_directory(dir_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from(dir_path));
    }

    #[test]
    fn test_validate_directory_does_not_exist() {
        let nonexistent_path = "/path/that/definitely/does/not/exist/12345";
        let result = parse_directory(nonexistent_path);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"));
        assert!(err.contains(nonexistent_path));
    }

    #[test]
    fn test_validate_directory_with_nested_structure() {
        let temp_dir = setup_test_dir();
        let nested_dir = temp_dir.path().join("nested").join("directory");
        std::fs::create_dir_all(&nested_dir).expect("Failed to create nested directories");

        let dir_path = nested_dir.to_str().unwrap();
        let result = parse_directory(dir_path);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), nested_dir);
    }

    #[test]
    fn test_validate_directory_with_special_chars() {
        let temp_dir = setup_test_dir();
        let special_dir = temp_dir.path().join("test with spaces and-symbols_!@#$");
        std::fs::create_dir(&special_dir)
            .expect("Failed to create directory with special characters");

        let dir_path = special_dir.to_str().unwrap();
        let result = parse_directory(dir_path);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), special_dir);
    }
}
