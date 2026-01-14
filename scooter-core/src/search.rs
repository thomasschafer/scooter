use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self};

use content_inspector::{ContentType, inspect};
use fancy_regex::Regex as FancyRegex;
use ignore::overrides::Override;
use ignore::{WalkBuilder, WalkState};
use regex::Regex;

use crate::{
    line_reader::{BufReadExt, LineEnding},
    replace::{self, ReplaceResult},
};

#[derive(Clone, Debug)]
pub enum Searcher {
    FileSearcher(FileSearcher),
    TextSearcher { search_config: ParsedSearchConfig },
}

impl Searcher {
    pub fn search(&self) -> &SearchType {
        match self {
            Self::FileSearcher(file_searcher) => file_searcher.search(),
            Self::TextSearcher { search_config } => &search_config.search,
        }
    }

    pub fn replace(&self) -> &str {
        match self {
            Self::FileSearcher(file_searcher) => file_searcher.replace(),
            Self::TextSearcher { search_config } => &search_config.replace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: Option<PathBuf>,
    /// 1-indexed
    pub line_number: usize,
    pub line: String,
    pub line_ending: LineEnding,
    pub included: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResultWithReplacement {
    pub search_result: SearchResult,
    pub replacement: String,
    pub replace_result: Option<ReplaceResult>,
}

impl SearchResultWithReplacement {
    pub fn display_error(&self) -> (String, &str) {
        let error = match &self.replace_result {
            Some(ReplaceResult::Error(error)) => error,
            None => panic!("Found error result with no error message"),
            Some(ReplaceResult::Success) => {
                panic!("Found successful result in errors: {self:?}")
            }
        };

        let path_display = format!(
            "{}:{}",
            self.search_result
                .path
                .clone()
                .unwrap_or_default()
                .display(),
            self.search_result.line_number
        );

        (path_display, error)
    }
}

#[derive(Clone, Debug)]
pub enum SearchType {
    Pattern(Regex),
    PatternAdvanced(FancyRegex),
    Fixed(String),
}

impl SearchType {
    pub fn is_empty(&self) -> bool {
        let str = match &self {
            SearchType::Pattern(r) => &r.to_string(),
            SearchType::PatternAdvanced(r) => &r.to_string(),
            SearchType::Fixed(s) => s,
        };
        str.is_empty()
    }
}

/// A function that processes search results for a file and determines whether to continue searching.
type FileVisitor = Box<dyn FnMut(Vec<SearchResult>) -> WalkState + Send>;

impl FileSearcher {
    pub fn search(&self) -> &SearchType {
        &self.search_config.search
    }

    pub fn replace(&self) -> &String {
        &self.search_config.replace
    }
}

/// Options for regex pattern conversion
#[derive(Clone, Debug)]
pub struct RegexOptions {
    /// Whether to match only whole words (bounded by non-word characters)
    pub whole_word: bool,
    /// Whether to perform case-sensitive matching
    pub match_case: bool,
}

#[derive(Clone, Debug)]
pub struct ParsedSearchConfig {
    /// The pattern to search for (fixed string or regex). Should be produced by `validation::parse_search_text`
    pub search: SearchType,
    /// The text to replace matches with
    pub replace: String,
}

#[derive(Clone, Debug)]
pub struct ParsedDirConfig {
    /// Configuration for file inclusion/exclusion patterns
    pub overrides: Override,
    /// The root directory to start searching from
    pub root_dir: PathBuf,
    /// Whether to include hidden files/directories in the search
    pub include_hidden: bool,
}

#[derive(Clone, Debug)]
pub struct FileSearcher {
    search_config: ParsedSearchConfig,
    dir_config: ParsedDirConfig,
}

impl FileSearcher {
    pub fn new(search_config: ParsedSearchConfig, dir_config: ParsedDirConfig) -> Self {
        Self {
            search_config,
            dir_config,
        }
    }

    fn build_walker(&self) -> ignore::WalkParallel {
        let num_threads = thread::available_parallelism()
            .map(NonZero::get)
            .unwrap_or(4)
            .min(12);

        WalkBuilder::new(&self.dir_config.root_dir)
            .hidden(!self.dir_config.include_hidden)
            .overrides(self.dir_config.overrides.clone())
            .threads(num_threads)
            .build_parallel()
    }

    /// Walks through files in the configured directory and processes matches.
    ///
    /// This method traverses the filesystem starting from the `root_dir` specified in the `FileSearcher`,
    /// respecting the configured overrides (include/exclude patterns) and hidden file settings.
    /// It uses parallel processing when possible for better performance.
    ///
    /// # Parameters
    ///
    /// * `cancelled` - An optional atomic boolean that can be used to signal cancellation from another thread.
    ///   If this is set to `true` during execution, the search will stop as soon as possible.
    ///
    /// * `file_handler` - A closure that returns a `FileVisitor`.
    ///   The returned `FileVisitor` is a function that processes search results for each file with matches.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::{
    ///     sync::{atomic::AtomicBool, mpsc},
    ///     path::PathBuf,
    /// };
    /// use regex::Regex;
    /// use ignore::{WalkState, overrides::Override};
    /// use scooter_core::search::{FileSearcher, ParsedSearchConfig, ParsedDirConfig, SearchResult, SearchType};
    ///
    /// let search_config = ParsedSearchConfig {
    ///     search: SearchType::Pattern(Regex::new("pattern").unwrap()),
    ///     replace: "replacement".to_string(),
    /// };
    /// let dir_config = ParsedDirConfig {
    ///     overrides: Override::empty(),
    ///     root_dir: PathBuf::from("."),
    ///     include_hidden: false,
    /// };
    /// let searcher = FileSearcher::new(search_config, dir_config);
    /// let cancelled = AtomicBool::new(false);
    ///
    /// searcher.walk_files(Some(&cancelled), move || {
    ///     Box::new(move |results| {
    ///         if process(results).is_err() {
    ///             WalkState::Quit
    ///         } else {
    ///             WalkState::Continue
    ///         }
    ///     })
    /// });
    ///
    /// fn process(results: Vec<SearchResult>) -> anyhow::Result<()> {
    ///     println!("{results:?}");
    ///     Ok(())
    /// }
    /// ```
    pub fn walk_files<F>(&self, cancelled: Option<&AtomicBool>, mut file_handler: F)
    where
        F: FnMut() -> FileVisitor + Send,
    {
        let walker = self.build_walker();
        walker.run(|| {
            let mut on_file_found = file_handler();
            Box::new(move |result| {
                if let Some(cancelled) = cancelled
                    && cancelled.load(Ordering::Relaxed)
                {
                    return WalkState::Quit;
                }

                let Ok(entry) = result else {
                    return WalkState::Continue;
                };

                if is_searchable(&entry) {
                    let results = match search_file(entry.path(), &self.search_config.search) {
                        Ok(r) => r,
                        Err(e) => {
                            log::warn!(
                                "Skipping {} due to error when searching: {e}",
                                entry.path().display()
                            );
                            return WalkState::Continue;
                        }
                    };

                    if !results.is_empty() {
                        return on_file_found(results);
                    }
                }
                WalkState::Continue
            })
        });
    }

    /// Walks through files in the configured directory and replaces matches.
    ///
    /// This method traverses the filesystem starting from the `root_dir` specified in the `FileSearcher`,
    /// respecting the configured overrides (include/exclude patterns) and hidden file settings.
    /// It replaces all matches of the search pattern with the replacement text in each file.
    ///
    /// # Parameters
    ///
    /// * `cancelled` - An optional atomic boolean that can be used to signal cancellation from another thread.
    ///   If this is set to `true` during execution, the search will stop as soon as possible.
    ///
    /// # Returns
    ///
    /// The number of files that had replacements performed in them.
    pub fn walk_files_and_replace(&self, cancelled: Option<&AtomicBool>) -> usize {
        let num_files_replaced_in = std::sync::Arc::new(AtomicUsize::new(0));

        let walker = self.build_walker();
        walker.run(|| {
            let counter = num_files_replaced_in.clone();

            Box::new(move |result| {
                if let Some(cancelled) = cancelled
                    && cancelled.load(Ordering::Relaxed)
                {
                    return WalkState::Quit;
                }

                let Ok(entry) = result else {
                    return WalkState::Continue;
                };

                if is_searchable(&entry) {
                    match replace::replace_all_in_file(entry.path(), self.search(), self.replace())
                    {
                        Ok(replaced_in_file) => {
                            if replaced_in_file {
                                counter.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            log::error!(
                                "Found error when performing replacement in {path_display}: {e}",
                                path_display = entry.path().display()
                            );
                        }
                    }
                }
                WalkState::Continue
            })
        });

        num_files_replaced_in.load(Ordering::Relaxed)
    }
}

const BINARY_EXTENSIONS: &[&str] = &[
    "png", "gif", "jpg", "jpeg", "ico", "svg", "pdf", "exe", "dll", "so", "bin", "class", "jar",
    "zip", "gz", "bz2", "xz", "7z", "tar",
];

fn is_likely_binary(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext_str| {
            BINARY_EXTENSIONS
                .iter()
                .any(|&bin_ext| ext_str.eq_ignore_ascii_case(bin_ext))
        })
}

fn is_searchable(entry: &ignore::DirEntry) -> bool {
    entry.file_type().is_some_and(|ft| ft.is_file()) && !is_likely_binary(entry.path())
}

pub fn contains_search(line: &str, search: &SearchType) -> bool {
    match search {
        SearchType::Fixed(fixed_str) => line.contains(fixed_str),
        SearchType::Pattern(pattern) => pattern.is_match(line),
        SearchType::PatternAdvanced(pattern) => pattern.is_match(line).is_ok_and(|r| r),
    }
}

pub fn search_file(path: &Path, search: &SearchType) -> anyhow::Result<Vec<SearchResult>> {
    if search.is_empty() {
        return Ok(vec![]);
    }
    let mut file = File::open(path)?;

    // Fast upfront binary sniff (8 KiB)
    let mut probe = [0u8; 8192];
    let read = file.read(&mut probe).unwrap_or(0);
    if matches!(inspect(&probe[..read]), ContentType::BINARY) {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(0))?;

    let reader = BufReader::with_capacity(16384, file);
    let mut results = Vec::new();

    let mut read_errors = 0;

    for (mut line_number, line_result) in reader.lines_with_endings().enumerate() {
        line_number += 1; // Ensure line-number is 1-indexed

        let (line_bytes, line_ending) = match line_result {
            Ok(l) => l,
            Err(err) => {
                read_errors += 1;
                log::warn!(
                    "Error retrieving line {line_number} of {}: {err}",
                    path.display()
                );
                #[allow(clippy::unnecessary_debug_formatting)]
                if read_errors >= 10 {
                    anyhow::bail!(
                        "Aborting search of {path:?}: too many read errors ({read_errors}). Most recent error: {err}",
                    );
                }
                continue;
            }
        };

        if let Ok(line) = String::from_utf8(line_bytes)
            && contains_search(&line, search)
        {
            let result = SearchResult {
                path: Some(path.to_path_buf()),
                line_number,
                line,
                line_ending,
                included: true,
            };
            results.push(result);
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod test_helpers {
        use super::*;

        pub fn create_test_search_result_with_replacement(
            path: &str,
            line_number: usize,
            replace_result: Option<ReplaceResult>,
        ) -> SearchResultWithReplacement {
            SearchResultWithReplacement {
                search_result: SearchResult {
                    path: Some(PathBuf::from(path)),
                    line_number,
                    line: "test line".to_string(),
                    line_ending: LineEnding::Lf,
                    included: true,
                },
                replacement: "replacement".to_string(),
                replace_result,
            }
        }

        pub fn create_fixed_search(term: &str) -> SearchType {
            SearchType::Fixed(term.to_string())
        }

        pub fn create_pattern_search(pattern: &str) -> SearchType {
            SearchType::Pattern(Regex::new(pattern).unwrap())
        }

        pub fn create_advanced_pattern_search(pattern: &str) -> SearchType {
            SearchType::PatternAdvanced(FancyRegex::new(pattern).unwrap())
        }
    }

    mod unicode_handling {
        use super::*;

        #[test]
        fn test_complex_unicode_replacement() {
            let text = "ASCII text with 世界 (CJK), Здравствуйте (Cyrillic), 안녕하세요 (Hangul), αβγδ (Greek), עִבְרִית (Hebrew)";
            let search = SearchType::Fixed("世界".to_string());

            let result = replace::replacement_if_match(text, &search, "World");

            assert_eq!(
                result,
                Some("ASCII text with World (CJK), Здравствуйте (Cyrillic), 안녕하세요 (Hangul), αβγδ (Greek), עִבְרִית (Hebrew)".to_string())
            );
        }

        #[test]
        fn test_unicode_normalization() {
            let text = "café";
            let search = SearchType::Fixed("é".to_string());
            assert_eq!(
                replace::replacement_if_match(text, &search, "e"),
                Some("cafe".to_string())
            );
        }

        #[test]
        fn test_unicode_regex_classes() {
            let text = "Latin A, Cyrillic Б, Greek Γ, Hebrew א";

            let search = SearchType::Pattern(Regex::new(r"\p{Cyrillic}").unwrap());
            assert_eq!(
                replace::replacement_if_match(text, &search, "X"),
                Some("Latin A, Cyrillic X, Greek Γ, Hebrew א".to_string())
            );

            let search = SearchType::Pattern(Regex::new(r"\p{Greek}").unwrap());
            assert_eq!(
                replace::replacement_if_match(text, &search, "X"),
                Some("Latin A, Cyrillic Б, Greek X, Hebrew א".to_string())
            );
        }

        #[test]
        fn test_unicode_capture_groups() {
            let text = "Name: 李明 (ID: A12345)";

            let search =
                SearchType::Pattern(Regex::new(r"Name: (\p{Han}+) \(ID: ([A-Z0-9]+)\)").unwrap());
            assert_eq!(
                replace::replacement_if_match(text, &search, "ID $2 belongs to $1"),
                Some("ID A12345 belongs to 李明".to_string())
            );
        }
    }

    mod replace_any {
        use super::*;

        #[test]
        fn test_simple_match_subword() {
            assert_eq!(
                replace::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Fixed("bar".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"bar").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foobarbaz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"bar").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
        }

        #[test]
        fn test_no_match() {
            assert_eq!(
                replace::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Fixed("xyz".to_string()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"xyz").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foobarbaz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"xyz").unwrap()),
                    "REPL"
                ),
                None
            );
        }

        #[test]
        fn test_word_boundaries() {
            assert_eq!(
                replace::replacement_if_match(
                    "foo bar baz",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "embargo",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foo bar baz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "embargo",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                None
            );
        }

        #[test]
        fn test_capture_groups() {
            assert_eq!(
                replace::replacement_if_match(
                    "John Doe",
                    &SearchType::Pattern(Regex::new(r"(\w+)\s+(\w+)").unwrap()),
                    "$2, $1"
                ),
                Some("Doe, John".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "John Doe",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"(\w+)\s+(\w+)").unwrap()),
                    "$2, $1"
                ),
                Some("Doe, John".to_string())
            );
        }

        #[test]
        fn test_lookaround() {
            assert_eq!(
                replace::replacement_if_match(
                    "123abc456",
                    &SearchType::PatternAdvanced(
                        FancyRegex::new(r"(?<=\d{3})abc(?=\d{3})").unwrap()
                    ),
                    "REPL"
                ),
                Some("123REPL456".to_string())
            );
        }

        #[test]
        fn test_quantifiers() {
            assert_eq!(
                replace::replacement_if_match(
                    "aaa123456bbb",
                    &SearchType::Pattern(Regex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "abc123def456",
                    &SearchType::Pattern(Regex::new(r"\d{3}").unwrap()),
                    "REPL"
                ),
                Some("abcREPLdefREPL".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "aaa123456bbb",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "abc123def456",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\d{3}").unwrap()),
                    "REPL"
                ),
                Some("abcREPLdefREPL".to_string())
            );
        }

        #[test]
        fn test_special_characters() {
            assert_eq!(
                replace::replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::Fixed(".bar*".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::Pattern(Regex::new(r"\.bar\*").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\.bar\*").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
        }

        #[test]
        fn test_unicode() {
            assert_eq!(
                replace::replacement_if_match(
                    "Hello 世界!",
                    &SearchType::Fixed("世界".to_string()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "Hello 世界!",
                    &SearchType::Pattern(Regex::new(r"世界").unwrap()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "Hello 世界!",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"世界").unwrap()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
        }

        #[test]
        fn test_case_insensitive() {
            assert_eq!(
                replace::replacement_if_match(
                    "HELLO world",
                    &SearchType::Pattern(Regex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
            assert_eq!(
                replace::replacement_if_match(
                    "HELLO world",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
        }
    }

    mod search_result_tests {
        use super::*;

        #[test]
        fn test_display_error_with_error_result() {
            let result = test_helpers::create_test_search_result_with_replacement(
                "/path/to/file.txt",
                42,
                Some(ReplaceResult::Error("Test error message".to_string())),
            );

            let (path_display, error) = result.display_error();

            assert_eq!(path_display, "/path/to/file.txt:42");
            assert_eq!(error, "Test error message");
        }

        #[test]
        fn test_display_error_with_unicode_path() {
            let result = test_helpers::create_test_search_result_with_replacement(
                "/path/to/файл.txt",
                123,
                Some(ReplaceResult::Error("Unicode test".to_string())),
            );

            let (path_display, error) = result.display_error();

            assert_eq!(path_display, "/path/to/файл.txt:123");
            assert_eq!(error, "Unicode test");
        }

        #[test]
        fn test_display_error_with_complex_error_message() {
            let complex_error = "Failed to write: Permission denied (os error 13)";
            let result = test_helpers::create_test_search_result_with_replacement(
                "/readonly/file.txt",
                1,
                Some(ReplaceResult::Error(complex_error.to_string())),
            );

            let (path_display, error) = result.display_error();

            assert_eq!(path_display, "/readonly/file.txt:1");
            assert_eq!(error, complex_error);
        }

        #[test]
        #[should_panic(expected = "Found error result with no error message")]
        fn test_display_error_panics_with_none_result() {
            let result = test_helpers::create_test_search_result_with_replacement(
                "/path/to/file.txt",
                1,
                None,
            );
            result.display_error();
        }

        #[test]
        #[should_panic(expected = "Found successful result in errors")]
        fn test_display_error_panics_with_success_result() {
            let result = test_helpers::create_test_search_result_with_replacement(
                "/path/to/file.txt",
                1,
                Some(ReplaceResult::Success),
            );
            result.display_error();
        }
    }

    mod search_type_tests {
        use super::*;

        #[test]
        fn test_search_type_emptiness() {
            let test_cases = [
                (test_helpers::create_fixed_search(""), true),
                (test_helpers::create_fixed_search("hello"), false),
                (test_helpers::create_fixed_search("   "), false), // whitespace is not empty
                (test_helpers::create_pattern_search(""), true),
                (test_helpers::create_pattern_search("test"), false),
                (test_helpers::create_pattern_search(r"\s+"), false),
                (test_helpers::create_advanced_pattern_search(""), true),
                (test_helpers::create_advanced_pattern_search("test"), false),
            ];

            for (search_type, expected_empty) in test_cases {
                assert_eq!(
                    search_type.is_empty(),
                    expected_empty,
                    "Emptiness test failed for: {search_type:?}"
                );
            }
        }
    }

    mod file_searcher_tests {
        use super::*;

        #[test]
        fn test_is_likely_binary_extensions() {
            const BINARY_EXTENSIONS: &[&str] = &[
                "image.png",
                "document.pdf",
                "archive.zip",
                "program.exe",
                "library.dll",
                "photo.jpg",
                "icon.ico",
                "vector.svg",
                "compressed.gz",
                "backup.7z",
                "java.class",
                "application.jar",
            ];

            const TEXT_EXTENSIONS: &[&str] = &[
                "code.rs",
                "script.py",
                "document.txt",
                "config.json",
                "readme.md",
                "style.css",
                "page.html",
                "source.c",
                "header.h",
                "makefile",
                "no_extension",
            ];

            const MIXED_CASE_BINARY: &[&str] =
                &["IMAGE.PNG", "Document.PDF", "ARCHIVE.ZIP", "Photo.JPG"];

            let test_cases = [
                (BINARY_EXTENSIONS, true),
                (TEXT_EXTENSIONS, false),
                (MIXED_CASE_BINARY, true),
            ];

            for (files, expected_binary) in test_cases {
                for file in files {
                    assert_eq!(
                        is_likely_binary(Path::new(file)),
                        expected_binary,
                        "Binary detection failed for {file}"
                    );
                }
            }
        }

        #[test]
        fn test_is_likely_binary_no_extension() {
            assert!(!is_likely_binary(Path::new("filename")));
            assert!(!is_likely_binary(Path::new("/path/to/file")));
        }

        #[test]
        fn test_is_likely_binary_empty_extension() {
            assert!(!is_likely_binary(Path::new("file.")));
        }

        #[test]
        fn test_is_likely_binary_complex_paths() {
            assert!(is_likely_binary(Path::new("/complex/path/to/image.png")));
            assert!(!is_likely_binary(Path::new("/complex/path/to/source.rs")));
        }

        #[test]
        fn test_is_likely_binary_hidden_files() {
            assert!(is_likely_binary(Path::new(".hidden.png")));
            assert!(!is_likely_binary(Path::new(".hidden.txt")));
        }
    }
}
