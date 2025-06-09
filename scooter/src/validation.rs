use crossterm::style::Stylize;
use fancy_regex::Regex as FancyRegex;
use ignore::{overrides::Override, overrides::OverrideBuilder};
use regex::Regex;
use scooter_core::search::{FileSearcher, FileSearcherConfig, SearchType};
use std::path::PathBuf;

use crate::utils;

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct SearchConfiguration {
    pub search_text: String,
    pub replacement_text: String,
    pub fixed_strings: bool,
    pub advanced_regex: bool,
    pub include_globs: String,
    pub exclude_globs: String,
    pub match_whole_word: bool,
    pub match_case: bool,
    pub include_hidden: bool,
    pub directory: PathBuf,
}

pub trait ValidationErrorHandler {
    fn handle_search_text_error(&mut self, error: &str, detail: &str);
    fn handle_include_files_error(&mut self, error: &str, detail: &str);
    fn handle_exclude_files_error(&mut self, error: &str, detail: &str);
}

/// Collects errors into an array
pub struct SimpleErrorHandler {
    pub errors: Vec<String>,
}

impl SimpleErrorHandler {
    pub fn new() -> Self {
        Self { errors: Vec::new() }
    }

    pub fn errors_str(&self) -> Option<String> {
        if self.errors.is_empty() {
            None
        } else {
            Some(format!("Validation errors:\n{}", self.errors.join("\n")))
        }
    }

    fn push_error(&mut self, err_msg: &str, detail: &str) {
        self.errors
            .push(format!("\n{title}:\n{detail}", title = err_msg.red()));
    }
}

impl Default for SimpleErrorHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationErrorHandler for SimpleErrorHandler {
    fn handle_search_text_error(&mut self, _error: &str, detail: &str) {
        self.push_error("Failed to parse search text", detail);
    }

    fn handle_include_files_error(&mut self, _error: &str, detail: &str) {
        self.push_error("Failed to parse include globs", detail);
    }

    fn handle_exclude_files_error(&mut self, _error: &str, detail: &str) {
        self.push_error("Failed to parse exclude globs", detail);
    }
}

#[derive(Debug)]
pub enum ValidationResult<T> {
    Success(T),
    ValidationErrors,
}

pub fn validate_search_configuration<H: ValidationErrorHandler>(
    config: SearchConfiguration,
    error_handler: &mut H,
) -> anyhow::Result<ValidationResult<FileSearcher>> {
    let search_pattern = parse_search_text(
        &config.search_text,
        config.fixed_strings,
        config.advanced_regex,
        error_handler,
    )?;

    let overrides = parse_overrides(
        &config.directory,
        &config.include_globs,
        &config.exclude_globs,
        error_handler,
    )?;

    match (search_pattern, overrides) {
        (ValidationResult::Success(search_pattern), ValidationResult::Success(overrides)) => {
            let searcher = FileSearcher::new(FileSearcherConfig {
                search: search_pattern,
                replace: config.replacement_text,
                whole_word: config.match_whole_word,
                match_case: config.match_case,
                overrides,
                root_dir: config.directory,
                include_hidden: config.include_hidden,
            });
            Ok(ValidationResult::Success(searcher))
        }
        _ => Ok(ValidationResult::ValidationErrors),
    }
}

fn parse_search_text_inner(
    search_text: &str,
    fixed_strings: bool,
    advanced_regex: bool,
) -> anyhow::Result<SearchType> {
    let result = if fixed_strings {
        SearchType::Fixed(search_text.to_string())
    } else if advanced_regex {
        SearchType::PatternAdvanced(FancyRegex::new(search_text)?)
    } else {
        SearchType::Pattern(Regex::new(search_text)?)
    };
    Ok(result)
}

fn parse_search_text<H: ValidationErrorHandler>(
    search_text: &str,
    fixed_strings: bool,
    advanced_regex: bool,
    error_handler: &mut H,
) -> anyhow::Result<ValidationResult<SearchType>> {
    match parse_search_text_inner(search_text, fixed_strings, advanced_regex) {
        Ok(pattern) => Ok(ValidationResult::Success(pattern)),
        Err(e) => {
            if utils::is_regex_error(&e) {
                error_handler.handle_search_text_error("Couldn't parse regex", &e.to_string());
                Ok(ValidationResult::ValidationErrors)
            } else {
                Err(e)
            }
        }
    }
}

fn parse_overrides<H: ValidationErrorHandler>(
    dir: &PathBuf,
    include_globs: &str,
    exclude_globs: &str,
    error_handler: &mut H,
) -> anyhow::Result<ValidationResult<Override>> {
    let mut overrides = OverrideBuilder::new(dir);
    let mut success = true;

    if let Err(e) = utils::add_overrides(&mut overrides, include_globs, "") {
        error_handler.handle_include_files_error("Couldn't parse glob pattern", &e.to_string());
        success = false;
    }

    if let Err(e) = utils::add_overrides(&mut overrides, exclude_globs, "!") {
        error_handler.handle_exclude_files_error("Couldn't parse glob pattern", &e.to_string());
        success = false;
    }

    if !success {
        return Ok(ValidationResult::ValidationErrors);
    }

    Ok(ValidationResult::Success(overrides.build()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config() -> SearchConfiguration {
        let temp_dir = TempDir::new().unwrap();
        SearchConfiguration {
            search_text: "test".to_string(),
            replacement_text: "replacement".to_string(),
            fixed_strings: false,
            advanced_regex: false,
            include_globs: "*.rs".to_string(),
            exclude_globs: "target/*".to_string(),
            match_whole_word: false,
            match_case: false,
            include_hidden: false,
            directory: temp_dir.path().to_path_buf(),
        }
    }

    #[test]
    fn test_valid_configuration() {
        let config = create_test_config();
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, &mut error_handler);

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), ValidationResult::Success(_)));
        assert!(error_handler.errors_str().is_none());
    }

    #[test]
    fn test_invalid_regex() {
        let mut config = create_test_config();
        config.search_text = "[invalid regex".to_string();
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, &mut error_handler);

        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            ValidationResult::ValidationErrors
        ));
        assert!(error_handler.errors_str().is_some());
        assert!(error_handler.errors[0].contains("Failed to parse search text"));
    }

    #[test]
    fn test_invalid_include_glob() {
        let mut config = create_test_config();
        config.include_globs = "[invalid".to_string();
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, &mut error_handler);

        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            ValidationResult::ValidationErrors
        ));
        assert!(error_handler.errors_str().is_some());
        assert!(error_handler.errors[0].contains("Failed to parse include globs"));
    }

    #[test]
    fn test_fixed_strings_mode() {
        let mut config = create_test_config();
        config.search_text = "[this would be invalid regex]".to_string();
        config.fixed_strings = true;
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, &mut error_handler);

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), ValidationResult::Success(_)));
        assert!(error_handler.errors_str().is_none());
    }
}
