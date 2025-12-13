use crossterm::style::Stylize;
use fancy_regex::Regex as FancyRegex;
use ignore::overrides::OverrideBuilder;
use regex::Regex;
use std::path::PathBuf;

use crate::search::{ParsedDirConfig, ParsedSearchConfig, SearchType};
use crate::utils;

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct SearchConfig<'a> {
    pub search_text: &'a str,
    pub replacement_text: &'a str,
    pub fixed_strings: bool,
    pub advanced_regex: bool,
    pub match_whole_word: bool,
    pub match_case: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirConfig<'a> {
    pub include_globs: Option<&'a str>,
    pub exclude_globs: Option<&'a str>,
    pub directory: PathBuf,
    pub include_hidden: bool,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationResult<T> {
    Success(T),
    ValidationErrors,
}

impl<T> ValidationResult<T> {
    fn map<U, F>(self, f: F) -> ValidationResult<U>
    where
        F: FnOnce(T) -> U,
        Self: Sized,
    {
        match self {
            ValidationResult::Success(t) => ValidationResult::Success(f(t)),
            ValidationResult::ValidationErrors => ValidationResult::ValidationErrors,
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
pub fn validate_search_configuration<H: ValidationErrorHandler>(
    search_config: SearchConfig<'_>,
    dir_config: Option<DirConfig<'_>>,
    error_handler: &mut H,
) -> anyhow::Result<ValidationResult<(ParsedSearchConfig, Option<ParsedDirConfig>)>> {
    let search_pattern = parse_search_text_with_error_handler(&search_config, error_handler)?;

    let parsed_dir_config = match dir_config {
        Some(dir_config) => {
            let overrides = parse_overrides(dir_config, error_handler)?;
            overrides.map(Some)
        }
        None => ValidationResult::Success(None),
    };

    if let (
        ValidationResult::Success(search_pattern),
        ValidationResult::Success(parsed_dir_config),
    ) = (search_pattern, parsed_dir_config)
    {
        let search_config = ParsedSearchConfig {
            search: search_pattern,
            replace: search_config.replacement_text.to_owned(),
        };
        Ok(ValidationResult::Success((
            search_config,
            parsed_dir_config,
        )))
    } else {
        Ok(ValidationResult::ValidationErrors)
    }
}

pub fn parse_search_text(config: &SearchConfig<'_>) -> anyhow::Result<SearchType> {
    if !config.match_whole_word && config.match_case {
        // No conversion required
        let search = if config.fixed_strings {
            SearchType::Fixed(config.search_text.to_string())
        } else if config.advanced_regex {
            SearchType::PatternAdvanced(FancyRegex::new(config.search_text)?)
        } else {
            SearchType::Pattern(Regex::new(config.search_text)?)
        };
        Ok(search)
    } else {
        let mut search_regex_str = if config.fixed_strings {
            regex::escape(config.search_text)
        } else {
            let search = config.search_text.to_owned();
            // Validate the regex without transformation
            FancyRegex::new(&search)?;
            search
        };

        if config.match_whole_word {
            search_regex_str = format!(r"(?<![a-zA-Z0-9_]){search_regex_str}(?![a-zA-Z0-9_])");
        }
        if !config.match_case {
            search_regex_str = format!(r"(?i){search_regex_str}");
        }

        // Shouldn't fail as we have already verified that the regex is valid, so `unwrap` here is fine.
        // (Any issues will likely be with the padding we are doing in this function.)
        let fancy_regex = FancyRegex::new(&search_regex_str).unwrap();
        Ok(SearchType::PatternAdvanced(fancy_regex))
    }
}

fn parse_search_text_with_error_handler<H: ValidationErrorHandler>(
    config: &SearchConfig<'_>,
    error_handler: &mut H,
) -> anyhow::Result<ValidationResult<SearchType>> {
    match parse_search_text(config) {
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
    dir_config: DirConfig<'_>,
    error_handler: &mut H,
) -> anyhow::Result<ValidationResult<ParsedDirConfig>> {
    let mut overrides = OverrideBuilder::new(&dir_config.directory);
    let mut success = true;

    if let Some(include_globs) = dir_config.include_globs
        && let Err(e) = utils::add_overrides(&mut overrides, include_globs, "")
    {
        error_handler.handle_include_files_error("Couldn't parse glob pattern", &e.to_string());
        success = false;
    }
    if let Some(exclude_globs) = dir_config.exclude_globs
        && let Err(e) = utils::add_overrides(&mut overrides, exclude_globs, "!")
    {
        error_handler.handle_exclude_files_error("Couldn't parse glob pattern", &e.to_string());
        success = false;
    }
    if !success {
        return Ok(ValidationResult::ValidationErrors);
    }

    Ok(ValidationResult::Success(ParsedDirConfig {
        overrides: overrides.build()?,
        root_dir: dir_config.directory,
        include_hidden: dir_config.include_hidden,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_search_test_config<'a>() -> SearchConfig<'a> {
        SearchConfig {
            search_text: "test",
            replacement_text: "replacement",
            fixed_strings: false,
            advanced_regex: false,
            match_whole_word: false,
            match_case: false,
        }
    }

    #[test]
    fn test_valid_configuration() {
        let config = create_search_test_config();
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, None, &mut error_handler);

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), ValidationResult::Success(_)));
        assert!(error_handler.errors_str().is_none());
    }

    #[test]
    fn test_invalid_regex() {
        let mut config = create_search_test_config();
        config.search_text = "[invalid regex";
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, None, &mut error_handler);

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
        let search_config = create_search_test_config();
        let dir_config = DirConfig {
            include_globs: Some("[invalid"),
            exclude_globs: None,
            directory: std::env::temp_dir(),
            include_hidden: false,
        };
        let mut error_handler = SimpleErrorHandler::new();

        let result =
            validate_search_configuration(search_config, Some(dir_config), &mut error_handler);

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
        let mut config = create_search_test_config();
        config.search_text = "[this would be invalid regex]";
        config.fixed_strings = true;
        let mut error_handler = SimpleErrorHandler::new();

        let result = validate_search_configuration(config, None, &mut error_handler);

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), ValidationResult::Success(_)));
        assert!(error_handler.errors_str().is_none());
    }

    mod parse_search_text_tests {
        use super::*;

        mod test_helpers {
            use super::*;

            pub fn assert_pattern_contains(search_type: &SearchType, expected_parts: &[&str]) {
                if let SearchType::PatternAdvanced(regex) = search_type {
                    let pattern = regex.as_str();
                    for part in expected_parts {
                        assert!(
                            pattern.contains(part),
                            "Pattern '{pattern}' should contain '{part}'"
                        );
                    }
                } else {
                    panic!("Expected PatternAdvanced, got {search_type:?}");
                }
            }
        }

        #[test]
        fn test_convert_regex_whole_word() {
            let search_config = SearchConfig {
                search_text: "test",
                replacement_text: "",
                fixed_strings: true,
                match_whole_word: true,
                match_case: true,
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();

            test_helpers::assert_pattern_contains(
                &converted,
                &["(?<![a-zA-Z0-9_])", "(?![a-zA-Z0-9_])", "test"],
            );
        }

        #[test]
        fn test_convert_regex_case_insensitive() {
            let search_config = SearchConfig {
                search_text: "Test",
                replacement_text: "",
                fixed_strings: true,
                match_whole_word: false,
                match_case: false,
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();

            test_helpers::assert_pattern_contains(&converted, &["(?i)", "Test"]);
        }

        #[test]
        fn test_convert_regex_whole_word_and_case_insensitive() {
            let search_config = SearchConfig {
                search_text: "Test",
                replacement_text: "",
                fixed_strings: true,
                match_whole_word: true,
                match_case: false,
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();

            test_helpers::assert_pattern_contains(
                &converted,
                &["(?<![a-zA-Z0-9_])", "(?![a-zA-Z0-9_])", "(?i)", "Test"],
            );
        }

        #[test]
        fn test_convert_regex_escapes_special_chars() {
            let search_config = SearchConfig {
                search_text: "test.regex*",
                replacement_text: "",
                fixed_strings: true,
                match_whole_word: true,
                match_case: true,
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();

            test_helpers::assert_pattern_contains(&converted, &[r"test\.regex\*"]);
        }

        #[test]
        fn test_convert_regex_from_existing_pattern() {
            let search_config = SearchConfig {
                search_text: r"\d+",
                replacement_text: "",
                fixed_strings: false,
                match_whole_word: true,
                match_case: false,
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();

            test_helpers::assert_pattern_contains(
                &converted,
                &["(?<![a-zA-Z0-9_])", "(?![a-zA-Z0-9_])", "(?i)", r"\d+"],
            );
        }

        #[test]
        fn test_fixed_string_with_unbalanced_paren_in_case_insensitive_mode() {
            let search_config = SearchConfig {
                search_text: "(foo",
                replacement_text: "",
                fixed_strings: true,
                match_whole_word: false,
                match_case: false, // forces regex wrapping
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();
            test_helpers::assert_pattern_contains(&converted, &[r"\(foo", "(?i)"]);
        }

        #[test]
        fn test_fixed_string_with_regex_chars_case_insensitive() {
            let search_config = SearchConfig {
                search_text: "test.regex*+?[chars]",
                replacement_text: "",
                fixed_strings: true,
                match_whole_word: false,
                match_case: false, // forces regex wrapping
                advanced_regex: false,
            };
            let converted = parse_search_text(&search_config).unwrap();
            test_helpers::assert_pattern_contains(
                &converted,
                &[r"test\.regex\*\+\?\[chars\]", "(?i)"],
            );
        }
    }
}
