use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self};

use content_inspector::{inspect, ContentType};
use fancy_regex::Regex as FancyRegex;
use ignore::overrides::Override;
use ignore::{WalkBuilder, WalkState};
use log::{error, warn};
use regex::Regex;

use crate::line_reader::{BufReadExt, LineEnding};
use crate::replace::ReplaceResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_number: usize,
    /// 1-indexed
    pub line: String,
    pub line_ending: LineEnding,
    pub replacement: String,
    pub included: bool,
    pub replace_result: Option<ReplaceResult>,
}

impl SearchResult {
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
            self.path
                .clone()
                .into_os_string()
                .into_string()
                .expect("Failed to display path"),
            self.line_number
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
    fn is_empty(&self) -> bool {
        let str = match &self {
            SearchType::Pattern(r) => &r.to_string(),
            SearchType::PatternAdvanced(r) => &r.to_string(),
            SearchType::Fixed(s) => s,
        };
        str.is_empty()
    }
}

type FileVisitor = Box<dyn FnMut(Vec<SearchResult>) -> WalkState + Send>;

#[derive(Clone, Debug)]
pub struct FileSearcher {
    pub search: SearchType,
    pub replace: String,
    pub overrides: Override,
    // TODO: `root_dir` and `include_hidden` are duplicated across this and App
    pub root_dir: PathBuf,
    pub include_hidden: bool,
}

impl FileSearcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search: SearchType,
        replace: String,
        // TODO: two bools in a row is a footgun
        whole_word: bool,
        match_case: bool,
        overrides: Override,
        root_dir: PathBuf,
        include_hidden: bool,
    ) -> Self {
        let search = if !whole_word && match_case {
            // No conversion required
            search
        } else {
            Self::convert_regex(&search, whole_word, match_case)
        };
        Self {
            search,
            replace,
            overrides,
            root_dir,
            include_hidden,
        }
    }

    fn convert_regex(search: &SearchType, whole_word: bool, match_case: bool) -> SearchType {
        let mut search_regex_str = match search {
            SearchType::Fixed(ref fixed_str) => regex::escape(fixed_str),
            SearchType::Pattern(ref pattern) => pattern.as_str().to_owned(),
            SearchType::PatternAdvanced(ref pattern) => pattern.as_str().to_owned(),
        };

        if whole_word {
            search_regex_str = format!(r"(?<![a-zA-Z0-9_]){search_regex_str}(?![a-zA-Z0-9_])");
        }
        if !match_case {
            search_regex_str = format!(r"(?i){search_regex_str}");
        }

        // Shouldn't fail as we have already verified that `search` is valid, so `unwrap` here is fine.
        // (Any issues will likely be with the padding we are doing in this function.)
        let fancy_regex = FancyRegex::new(&search_regex_str).unwrap();
        SearchType::PatternAdvanced(fancy_regex)
    }

    // TODO: document
    pub fn walk_files<F>(&self, cancelled: &AtomicBool, mut file_handler: F)
    where
        F: FnMut() -> FileVisitor + Send,
    {
        cancelled.store(false, Ordering::Relaxed);

        let num_threads = thread::available_parallelism()
            .map(NonZero::get)
            .unwrap_or(4)
            .min(12);

        let walker = WalkBuilder::new(&self.root_dir)
            .hidden(!self.include_hidden)
            .overrides(self.overrides.clone())
            .threads(num_threads)
            .build_parallel();

        walker.run(|| {
            let mut on_file_found = file_handler();
            Box::new(move |result| {
                if cancelled.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }

                let Ok(entry) = result else {
                    return WalkState::Continue;
                };

                #[allow(clippy::collapsible_if)]
                if entry.file_type().is_some_and(|ft| ft.is_file())
                    && !Self::is_likely_binary(entry.path())
                {
                    let results = Self::search_file(entry.path(), &self.search, &self.replace);
                    if let Some(results) = results {
                        if !results.is_empty() {
                            return on_file_found(results);
                        }
                    }
                }
                WalkState::Continue
            })
        });
    }

    // TODO: return result, not option
    fn search_file(path: &Path, search: &SearchType, replace: &str) -> Option<Vec<SearchResult>> {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(err) => {
                warn!("Error opening file {}: {err}", path.display());
                return None;
            }
        };

        // Fast upfront binary sniff (8 KiB)
        let mut probe = [0u8; 8192];
        let read = file.read(&mut probe).unwrap_or(0);
        if matches!(inspect(&probe[..read]), ContentType::BINARY) {
            return None;
        }
        if file.seek(SeekFrom::Start(0)).is_err() {
            error!("Failed to seek file {} to start", path.display());
            return None;
        }

        let reader = BufReader::with_capacity(16384, file);
        let mut results = Vec::new();

        let mut read_errors = 0;

        for (mut line_number, line_result) in reader.lines_with_endings().enumerate() {
            line_number += 1; // Ensure line-number is 1-indexed

            let (line_bytes, line_ending) = match line_result {
                Ok(l) => l,
                Err(err) => {
                    read_errors += 1;
                    warn!(
                        "Error retrieving line {line_number} of {}: {err}",
                        path.display()
                    );
                    if read_errors >= 10 {
                        break;
                    }
                    continue;
                }
            };

            if let Ok(line) = String::from_utf8(line_bytes) {
                if let Some(replacement) = Self::replacement_if_match(&line, search, replace) {
                    let result = SearchResult {
                        path: path.to_path_buf(),
                        line_number,
                        line,
                        line_ending,
                        replacement,
                        included: true,
                        replace_result: None,
                    };
                    results.push(result);
                }
            }
        }

        Some(results)
    }

    fn replacement_if_match(line: &str, search: &SearchType, replace: &str) -> Option<String> {
        if line.is_empty() || search.is_empty() {
            return None;
        }

        match search {
            SearchType::Fixed(ref fixed_str) => {
                if line.contains(fixed_str) {
                    Some(line.replace(fixed_str, replace))
                } else {
                    None
                }
            }
            SearchType::Pattern(ref pattern) => {
                if pattern.is_match(line) {
                    Some(pattern.replace_all(line, replace).to_string())
                } else {
                    None
                }
            }
            SearchType::PatternAdvanced(ref pattern) => match pattern.is_match(line) {
                Ok(true) => Some(pattern.replace_all(line, replace).to_string()),
                _ => None,
            },
        }
    }

    fn is_likely_binary(path: &Path) -> bool {
        const BINARY_EXTENSIONS: &[&str] = &[
            "png", "gif", "jpg", "jpeg", "ico", "svg", "pdf", "exe", "dll", "so", "bin", "class",
            "jar", "zip", "gz", "bz2", "xz", "7z", "tar",
        ];
        if let Some(ext) = path.extension() {
            if let Some(ext_str) = ext.to_str() {
                return BINARY_EXTENSIONS.contains(&ext_str.to_lowercase().as_str());
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test helper functions to reduce duplication
    mod test_helpers {
        use super::*;

        pub fn create_test_search_result(
            path: &str,
            line_number: usize,
            replace_result: Option<ReplaceResult>,
        ) -> SearchResult {
            SearchResult {
                path: PathBuf::from(path),
                line_number,
                line: "test line".to_string(),
                line_ending: LineEnding::Lf,
                replacement: "replacement".to_string(),
                included: true,
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

    mod replace_whole_word {
        use super::*;

        #[test]
        fn test_basic_replacement() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "hello world",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                Some("hello earth".to_string())
            );
        }

        #[test]
        fn test_multiple_replacements() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "world hello world",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                Some("earth hello earth".to_string())
            );
        }

        #[test]
        fn test_no_match() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "worldwide",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "_world_",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_word_boundaries() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    ",world-",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                Some(",earth-".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "world-word",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                Some("earth-word".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "Hello-world!",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                Some("Hello-earth!".to_string())
            );
        }

        #[test]
        fn test_case_sensitive() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "Hello WORLD",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        true
                    ),
                    "earth"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "Hello world",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("wOrld".to_string()),
                        true,
                        true
                    ),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_empty_strings() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "hello world",
                    &FileSearcher::convert_regex(&SearchType::Fixed("".to_string()), true, false),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_substring_no_match() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "worldwide web",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "underworld",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_special_regex_chars() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "hello (world)",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("(world)".to_string()),
                        true,
                        false
                    ),
                    "earth"
                ),
                Some("hello earth".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "hello world.*",
                    &FileSearcher::convert_regex(
                        &SearchType::Fixed("world.*".to_string()),
                        true,
                        false
                    ),
                    "ea+rth"
                ),
                Some("hello ea+rth".to_string())
            );
        }

        #[test]
        fn test_basic_regex_patterns() {
            let re = Regex::new(r"ax*b").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foo axxxxb bar",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "NEW"
                ),
                Some("foo NEW bar".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "fooaxxxxb bar",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_patterns_with_spaces() {
            let re = Regex::new(r"hel+o world").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "say hello world!",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "hi earth"
                ),
                Some("say hi earth!".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "helloworld",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "hi earth"
                ),
                None
            );
        }

        #[test]
        fn test_multiple_matches() {
            let re = Regex::new(r"a+b+").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foo aab abb",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("foo X X".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "ab abaab abb",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("X abaab X".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "ababaababb",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "ab ab aab abb",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("X X X X".to_string())
            );
        }

        #[test]
        fn test_boundary_cases() {
            let re = Regex::new(r"foo\s*bar").unwrap();
            // At start of string
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foo bar baz",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "TEST"
                ),
                Some("TEST baz".to_string())
            );
            // At end of string
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "baz foo bar",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "TEST"
                ),
                Some("baz TEST".to_string())
            );
            // With punctuation
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "(foo bar)",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "TEST"
                ),
                Some("(TEST)".to_string())
            );
        }

        #[test]
        fn test_with_punctuation() {
            let re = Regex::new(r"a\d+b").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "(a123b)",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("(X)".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foo.a123b!bar",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("foo.X!bar".to_string())
            );
        }

        #[test]
        fn test_complex_patterns() {
            let re = Regex::new(r"[a-z]+\d+[a-z]+").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "test9 abc123def 8xyz",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "NEW"
                ),
                Some("test9 NEW 8xyz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "test9abc123def8xyz",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_optional_patterns() {
            let re = Regex::new(r"colou?r").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "my color and colour",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("my X and X".to_string())
            );
        }

        #[test]
        fn test_empty_haystack() {
            let re = Regex::new(r"test").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_empty_search_regex() {
            let re = Regex::new(r"").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "search",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_single_char() {
            let re = Regex::new(r"a").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "b a c",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("b X c".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "bac",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                None
            );
        }

        #[test]
        fn test_escaped_chars() {
            let re = Regex::new(r"\(\d+\)").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "test (123) foo",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("test X foo".to_string())
            );
        }

        #[test]
        fn test_with_unicode() {
            let re = Regex::new(r"λ\d+").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "calc λ123 β",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("calc X β".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "calcλ123",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                None
            );
        }

        #[test]
        fn test_multiline_patterns() {
            let re = Regex::new(r"foo\s*\n\s*bar").unwrap();
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "test foo\nbar end",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "NEW"
                ),
                Some("test NEW end".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "test foo\n  bar end",
                    &FileSearcher::convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                Some("test NEW end".to_string())
            );
        }
    }

    mod replace_any {
        use super::*;

        #[test]
        fn test_simple_match_subword() {
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Fixed("bar".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"bar").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Fixed("xyz".to_string()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"xyz").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "foo bar baz",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "embargo",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foo bar baz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "John Doe",
                    &SearchType::Pattern(Regex::new(r"(\w+)\s+(\w+)").unwrap()),
                    "$2, $1"
                ),
                Some("Doe, John".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "aaa123456bbb",
                    &SearchType::Pattern(Regex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "abc123def456",
                    &SearchType::Pattern(Regex::new(r"\d{3}").unwrap()),
                    "REPL"
                ),
                Some("abcREPLdefREPL".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "aaa123456bbb",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::Fixed(".bar*".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::Pattern(Regex::new(r"\.bar\*").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "Hello 世界!",
                    &SearchType::Fixed("世界".to_string()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
                    "Hello 世界!",
                    &SearchType::Pattern(Regex::new(r"世界").unwrap()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
                FileSearcher::replacement_if_match(
                    "HELLO world",
                    &SearchType::Pattern(Regex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
            assert_eq!(
                FileSearcher::replacement_if_match(
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
            let result = test_helpers::create_test_search_result(
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
            let result = test_helpers::create_test_search_result(
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
            let result = test_helpers::create_test_search_result(
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
            let result = test_helpers::create_test_search_result("/path/to/file.txt", 1, None);
            result.display_error();
        }

        #[test]
        #[should_panic(expected = "Found successful result in errors")]
        fn test_display_error_panics_with_success_result() {
            let result = test_helpers::create_test_search_result(
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
        use ignore::overrides::OverrideBuilder;

        fn create_test_override() -> Override {
            OverrideBuilder::new("/tmp").build().unwrap()
        }

        #[test]
        fn test_new_no_conversion_needed() {
            let search = test_helpers::create_fixed_search("test");
            let searcher = FileSearcher::new(
                search.clone(),
                "replacement".to_string(),
                false, // whole_word
                true,  // match_case
                create_test_override(),
                PathBuf::from("/tmp"),
                false,
            );

            // When whole_word=false and match_case=true, no conversion should happen
            // We can't directly compare SearchType due to regex internals, but we can check the replace string
            assert_eq!(searcher.replace, "replacement");
            assert_eq!(searcher.root_dir, PathBuf::from("/tmp"));
            assert!(!searcher.include_hidden);
        }

        #[test]
        fn test_new_with_conversion() {
            let search = test_helpers::create_fixed_search("test");
            let searcher = FileSearcher::new(
                search,
                "replacement".to_string(),
                true,  // whole_word - should trigger conversion
                false, // match_case - should trigger conversion
                create_test_override(),
                PathBuf::from("/home"),
                true, // include_hidden
            );

            assert_eq!(searcher.replace, "replacement");
            assert_eq!(searcher.root_dir, PathBuf::from("/home"));
            assert!(searcher.include_hidden);
            // The search should have been converted to PatternAdvanced
            assert!(matches!(searcher.search, SearchType::PatternAdvanced(_)));
        }

        #[test]
        fn test_convert_regex_whole_word() {
            let fixed_search = test_helpers::create_fixed_search("test");
            let converted = FileSearcher::convert_regex(&fixed_search, true, true);

            test_helpers::assert_pattern_contains(
                &converted,
                &["(?<![a-zA-Z0-9_])", "(?![a-zA-Z0-9_])", "test"],
            );
        }

        #[test]
        fn test_convert_regex_case_insensitive() {
            let fixed_search = test_helpers::create_fixed_search("Test");
            let converted = FileSearcher::convert_regex(&fixed_search, false, false);

            test_helpers::assert_pattern_contains(&converted, &["(?i)", "Test"]);
        }

        #[test]
        fn test_convert_regex_whole_word_and_case_insensitive() {
            let fixed_search = test_helpers::create_fixed_search("Test");
            let converted = FileSearcher::convert_regex(&fixed_search, true, false);

            test_helpers::assert_pattern_contains(
                &converted,
                &["(?<![a-zA-Z0-9_])", "(?![a-zA-Z0-9_])", "(?i)", "Test"],
            );
        }

        #[test]
        fn test_convert_regex_escapes_special_chars() {
            let fixed_search = test_helpers::create_fixed_search("test.regex*");
            let converted = FileSearcher::convert_regex(&fixed_search, false, true);

            test_helpers::assert_pattern_contains(&converted, &[r"test\.regex\*"]);
        }

        #[test]
        fn test_convert_regex_from_existing_pattern() {
            let pattern_search = test_helpers::create_pattern_search(r"\d+");
            let converted = FileSearcher::convert_regex(&pattern_search, true, false);

            test_helpers::assert_pattern_contains(
                &converted,
                &["(?<![a-zA-Z0-9_])", "(?![a-zA-Z0-9_])", "(?i)", r"\d+"],
            );
        }

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
                        FileSearcher::is_likely_binary(Path::new(file)),
                        expected_binary,
                        "Binary detection failed for {file}"
                    );
                }
            }
        }

        #[test]
        fn test_is_likely_binary_no_extension() {
            assert!(!FileSearcher::is_likely_binary(Path::new("filename")));
            assert!(!FileSearcher::is_likely_binary(Path::new("/path/to/file")));
        }

        #[test]
        fn test_is_likely_binary_empty_extension() {
            assert!(!FileSearcher::is_likely_binary(Path::new("file.")));
        }

        #[test]
        fn test_is_likely_binary_complex_paths() {
            assert!(FileSearcher::is_likely_binary(Path::new(
                "/complex/path/to/image.png"
            )));
            assert!(!FileSearcher::is_likely_binary(Path::new(
                "/complex/path/to/source.rs"
            )));
        }

        #[test]
        fn test_is_likely_binary_hidden_files() {
            assert!(FileSearcher::is_likely_binary(Path::new(".hidden.png")));
            assert!(!FileSearcher::is_likely_binary(Path::new(".hidden.txt")));
        }
    }

    mod search_file_tests {
        use super::*;
        use std::io::Write;
        use tempfile::NamedTempFile;

        #[test]
        fn test_search_file_simple_match() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "line 1").unwrap();
            writeln!(temp_file, "search target").unwrap();
            writeln!(temp_file, "line 3").unwrap();
            temp_file.flush().unwrap();

            let search = test_helpers::create_fixed_search("search");
            let results = FileSearcher::search_file(temp_file.path(), &search, "replace").unwrap();

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].line_number, 2);
            assert_eq!(results[0].line, "search target");
            assert_eq!(results[0].replacement, "replace target");
            assert!(results[0].included);
            assert!(results[0].replace_result.is_none());
        }

        #[test]
        fn test_search_file_multiple_matches() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "test line 1").unwrap();
            writeln!(temp_file, "test line 2").unwrap();
            writeln!(temp_file, "no match here").unwrap();
            writeln!(temp_file, "test line 4").unwrap();
            temp_file.flush().unwrap();

            let search = test_helpers::create_fixed_search("test");
            let results = FileSearcher::search_file(temp_file.path(), &search, "replaced").unwrap();

            assert_eq!(results.len(), 3);
            assert_eq!(results[0].line_number, 1);
            assert_eq!(results[0].replacement, "replaced line 1");
            assert_eq!(results[1].line_number, 2);
            assert_eq!(results[1].replacement, "replaced line 2");
            assert_eq!(results[2].line_number, 4);
            assert_eq!(results[2].replacement, "replaced line 4");
        }

        #[test]
        fn test_search_file_no_matches() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "line 1").unwrap();
            writeln!(temp_file, "line 2").unwrap();
            writeln!(temp_file, "line 3").unwrap();
            temp_file.flush().unwrap();

            let search = SearchType::Fixed("nonexistent".to_string());
            let results = FileSearcher::search_file(temp_file.path(), &search, "replace").unwrap();

            assert_eq!(results.len(), 0);
        }

        #[test]
        fn test_search_file_regex_pattern() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "number: 123").unwrap();
            writeln!(temp_file, "text without numbers").unwrap();
            writeln!(temp_file, "another number: 456").unwrap();
            temp_file.flush().unwrap();

            let search = SearchType::Pattern(Regex::new(r"\d+").unwrap());
            let results = FileSearcher::search_file(temp_file.path(), &search, "XXX").unwrap();

            assert_eq!(results.len(), 2);
            assert_eq!(results[0].replacement, "number: XXX");
            assert_eq!(results[1].replacement, "another number: XXX");
        }

        #[test]
        fn test_search_file_advanced_regex_pattern() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "123abc456").unwrap();
            writeln!(temp_file, "abc").unwrap();
            writeln!(temp_file, "789xyz123").unwrap();
            writeln!(temp_file, "no match").unwrap();
            temp_file.flush().unwrap();

            // Positive lookbehind and lookahead
            let search =
                SearchType::PatternAdvanced(FancyRegex::new(r"(?<=\d{3})abc(?=\d{3})").unwrap());
            let results = FileSearcher::search_file(temp_file.path(), &search, "REPLACED").unwrap();

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].replacement, "123REPLACED456");
            assert_eq!(results[0].line_number, 1);
        }

        #[test]
        fn test_search_file_empty_search() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "some content").unwrap();
            temp_file.flush().unwrap();

            let search = SearchType::Fixed("".to_string());
            let results = FileSearcher::search_file(temp_file.path(), &search, "replace").unwrap();

            assert_eq!(results.len(), 0);
        }

        #[test]
        fn test_search_file_preserves_line_endings() {
            let mut temp_file = NamedTempFile::new().unwrap();
            write!(temp_file, "line1\nline2\r\nline3").unwrap();
            temp_file.flush().unwrap();

            let search = SearchType::Fixed("line".to_string());
            let results = FileSearcher::search_file(temp_file.path(), &search, "X").unwrap();

            assert_eq!(results.len(), 3);
            assert_eq!(results[0].line_ending, LineEnding::Lf);
            assert_eq!(results[1].line_ending, LineEnding::CrLf);
            assert_eq!(results[2].line_ending, LineEnding::None);
        }

        #[test]
        fn test_search_file_nonexistent() {
            let nonexistent_path = PathBuf::from("/this/file/does/not/exist.txt");
            let search = test_helpers::create_fixed_search("test");
            let results = FileSearcher::search_file(&nonexistent_path, &search, "replace");

            assert!(results.is_none());
        }

        #[test]
        fn test_search_file_unicode_content() {
            let mut temp_file = NamedTempFile::new().unwrap();
            writeln!(temp_file, "Hello 世界!").unwrap();
            writeln!(temp_file, "Здравствуй мир!").unwrap();
            writeln!(temp_file, "🚀 Rocket").unwrap();
            temp_file.flush().unwrap();

            let search = SearchType::Fixed("世界".to_string());
            let results = FileSearcher::search_file(temp_file.path(), &search, "World").unwrap();

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].replacement, "Hello World!");
        }

        #[test]
        fn test_search_file_with_binary_content() {
            let mut temp_file = NamedTempFile::new().unwrap();
            // Write some binary data (null bytes and other control characters)
            let binary_data = [0x00, 0x01, 0x02, 0xFF, 0xFE];
            temp_file.write_all(&binary_data).unwrap();
            temp_file.flush().unwrap();

            let search = test_helpers::create_fixed_search("test");
            let results = FileSearcher::search_file(temp_file.path(), &search, "replace");

            assert!(results.is_none());
        }

        #[test]
        fn test_search_file_large_content() {
            let mut temp_file = NamedTempFile::new().unwrap();

            // Write a large file with search targets scattered throughout
            for i in 0..1000 {
                if i % 100 == 0 {
                    writeln!(temp_file, "target line {i}").unwrap();
                } else {
                    writeln!(temp_file, "normal line {i}").unwrap();
                }
            }
            temp_file.flush().unwrap();

            let search = SearchType::Fixed("target".to_string());
            let results = FileSearcher::search_file(temp_file.path(), &search, "found").unwrap();

            assert_eq!(results.len(), 10); // Lines 0, 100, 200, ..., 900
            assert_eq!(results[0].line_number, 1); // 1-indexed
            assert_eq!(results[1].line_number, 101);
            assert_eq!(results[9].line_number, 901);
        }
    }
}
