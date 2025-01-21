use content_inspector::{inspect, ContentType};
use fancy_regex::Regex as FancyRegex;
use ignore::{WalkBuilder, WalkParallel};
use log::warn;
use regex::Regex;
use std::path::{Path, PathBuf};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::{fs::File, io::BufReader};

use crate::{
    app::{BackgroundProcessingEvent, SearchResult},
    utils::relative_path_from,
};

fn replace_whole_word_if_match_regex(
    haystack: &str,
    search: &FancyRegexWithBoundaries,
    replacement: &str,
) -> Option<String> {
    let FancyRegexWithBoundaries(search) = search;

    if haystack.is_empty() || search.to_string().is_empty() {
        return None;
    }

    if search.is_match(haystack).unwrap() {
        let replaced = search.replace_all(haystack, replacement);
        Some(replaced.to_string())
    } else {
        None
    }
}

fn replacement_if_match_any(line: &str, search: &SearchType, replacement: &str) -> Option<String> {
    match search {
        SearchType::Fixed(ref fixed_str) => {
            if line.contains(fixed_str) {
                Some(line.replace(fixed_str, replacement))
            } else {
                None
            }
        }
        SearchType::Pattern(ref pattern) => {
            if pattern.is_match(line) {
                Some(pattern.replace_all(line, replacement).to_string())
            } else {
                None
            }
        }
        SearchType::PatternAdvanced(ref pattern) => match pattern.is_match(line) {
            Ok(true) => Some(pattern.replace_all(line, replacement).to_string()),
            _ => None,
        },
    }
}

#[derive(Clone, Debug)]
pub enum SearchType {
    Pattern(Regex),
    PatternAdvanced(FancyRegex),
    Fixed(String),
}

fn add_boundaries(search: SearchType) -> FancyRegexWithBoundaries {
    let search_str = match search {
        SearchType::Fixed(ref fixed_str) => &regex::escape(fixed_str),

        SearchType::Pattern(ref pattern) => pattern.as_str(),
        SearchType::PatternAdvanced(ref pattern) => pattern.as_str(),
    };

    let pattern_with_boundaries =
        FancyRegex::new(&format!(r"(?<![a-zA-Z0-9_]){}(?![a-zA-Z0-9_])", search_str)).unwrap();
    FancyRegexWithBoundaries(pattern_with_boundaries)
}

#[derive(Clone, Debug)]
struct FancyRegexWithBoundaries(FancyRegex);

#[derive(Clone, Debug)]
enum SearchMatchType {
    Any(SearchType),
    WholeWord(FancyRegexWithBoundaries),
}

#[derive(Clone, Debug)]
pub struct ParsedFields {
    search_match_type: SearchMatchType,
    replace: String,
    path_pattern: Option<SearchType>,
    // TODO: `root_dir` and `include_hidden` are duplicated across this and App
    root_dir: PathBuf,
    include_hidden: bool,

    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
}

impl ParsedFields {
    // TODO: add tests for instantiating and handling paths
    pub fn new(
        search: SearchType,
        replace: String,
        whole_word: bool,
        path_pattern: Option<SearchType>,
        root_dir: PathBuf,
        include_hidden: bool,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> Self {
        let search_match_type = if whole_word {
            SearchMatchType::WholeWord(add_boundaries(search))
        } else {
            SearchMatchType::Any(search)
        };
        Self {
            search_match_type,
            replace,
            path_pattern,
            root_dir,
            include_hidden,
            background_processing_sender,
        }
    }

    pub async fn handle_path(&self, path: &Path) {
        if let Some(ref p) = self.path_pattern {
            if !self.matches_pattern(path, p) {
                return;
            }
        }

        match File::open(path).await {
            Ok(file) => {
                let reader = BufReader::new(file);

                let mut lines = reader.lines();
                let mut line_number = 0;
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if let ContentType::BINARY = inspect(line.as_bytes()) {
                                continue;
                            }
                            if let Some(replacement) = self.replacement_if_match(line.clone()) {
                                let search_result = SearchResult {
                                    path: path.to_path_buf(),
                                    line_number: line_number + 1,
                                    line: line.clone(),
                                    replacement,
                                    included: true,
                                    replace_result: None,
                                };
                                let send_result = self.background_processing_sender.send(
                                    BackgroundProcessingEvent::AddSearchResult(search_result),
                                );
                                if send_result.is_err() {
                                    // likely state reset, thread about to be killed
                                    return;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(err) => {
                            warn!("Error retrieving line {} of {:?}: {err}", line_number, path);
                        }
                    }
                    line_number += 1;
                }
            }
            Err(err) => {
                warn!("Error opening file {:?}: {err}", path);
            }
        }
    }

    fn matches_pattern(&self, path: &Path, p: &SearchType) -> bool {
        let relative_path = relative_path_from(&self.root_dir, path);
        let relative_path = relative_path.as_str();

        match p {
            SearchType::Pattern(ref p) => p.is_match(relative_path),
            SearchType::PatternAdvanced(ref p) => p.is_match(relative_path).unwrap(),
            SearchType::Fixed(ref s) => relative_path.contains(s),
        }
    }

    fn replacement_if_match(&self, line: String) -> Option<String> {
        match &self.search_match_type {
            SearchMatchType::Any(search) => replacement_if_match_any(&line, search, &self.replace),
            SearchMatchType::WholeWord(search) => {
                replace_whole_word_if_match_regex(&line, search, &self.replace)
            }
        }
    }

    pub(crate) fn build_walker(&self) -> WalkParallel {
        WalkBuilder::new(&self.root_dir)
            .hidden(!self.include_hidden)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build_parallel()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod replace_whole_word {
        use super::*;

        #[test]
        fn test_basic_replacement() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "hello world",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                Some("hello earth".to_string())
            );
        }

        #[test]
        fn test_multiple_replacements() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "world hello world",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                Some("earth hello earth".to_string())
            );
        }

        #[test]
        fn test_no_match() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "worldwide",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "_world_",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_word_boundaries() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    ",world-",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                Some(",earth-".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "world-word",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                Some("earth-word".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "Hello-world!",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                Some("Hello-earth!".to_string())
            );
        }

        #[test]
        fn test_case_sensitive() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "Hello WORLD",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "Hello world",
                    &add_boundaries(SearchType::Fixed("wOrld".to_string())),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_empty_strings() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "hello world",
                    &add_boundaries(SearchType::Fixed("".to_string())),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_substring_no_match() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "worldwide web",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "underworld",
                    &add_boundaries(SearchType::Fixed("world".to_string())),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_special_regex_chars() {
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "hello (world)",
                    &add_boundaries(SearchType::Fixed("(world)".to_string())),
                    "earth"
                ),
                Some("hello earth".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "hello world.*",
                    &add_boundaries(SearchType::Fixed("world.*".to_string())),
                    "ea+rth"
                ),
                Some("hello ea+rth".to_string())
            );
        }

        #[test]
        fn test_basic_regex_patterns() {
            let re = Regex::new(r"ax*b").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "foo axxxxb bar",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "NEW"
                ),
                Some("foo NEW bar".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "fooaxxxxb bar",
                    &add_boundaries(SearchType::Pattern(re)),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_patterns_with_spaces() {
            let re = Regex::new(r"hel+o world").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "say hello world!",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "hi earth"
                ),
                Some("say hi earth!".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "helloworld",
                    &add_boundaries(SearchType::Pattern(re)),
                    "hi earth"
                ),
                None
            );
        }

        #[test]
        fn test_multiple_matches() {
            let re = Regex::new(r"a+b+").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "foo aab abb",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "X"
                ),
                Some("foo X X".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "ab abaab abb",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "X"
                ),
                Some("X abaab X".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "ababaababb",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "X"
                ),
                None
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "ab ab aab abb",
                    &add_boundaries(SearchType::Pattern(re)),
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
                replace_whole_word_if_match_regex(
                    "foo bar baz",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "TEST"
                ),
                Some("TEST baz".to_string())
            );
            // At end of string
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "baz foo bar",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "TEST"
                ),
                Some("baz TEST".to_string())
            );
            // With punctuation
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "(foo bar)",
                    &add_boundaries(SearchType::Pattern(re)),
                    "TEST"
                ),
                Some("(TEST)".to_string())
            );
        }

        #[test]
        fn test_with_punctuation() {
            let re = Regex::new(r"a\d+b").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "(a123b)",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "X"
                ),
                Some("(X)".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "foo.a123b!bar",
                    &add_boundaries(SearchType::Pattern(re)),
                    "X"
                ),
                Some("foo.X!bar".to_string())
            );
        }

        #[test]
        fn test_complex_patterns() {
            let re = Regex::new(r"[a-z]+\d+[a-z]+").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "test9 abc123def 8xyz",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "NEW"
                ),
                Some("test9 NEW 8xyz".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "test9abc123def8xyz",
                    &add_boundaries(SearchType::Pattern(re)),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_optional_patterns() {
            let re = Regex::new(r"colou?r").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "my color and colour",
                    &add_boundaries(SearchType::Pattern(re)),
                    "X"
                ),
                Some("my X and X".to_string())
            );
        }

        #[test]
        fn test_empty_haystack() {
            let re = Regex::new(r"test").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "",
                    &add_boundaries(SearchType::Pattern(re)),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_empty_search_regex() {
            let re = Regex::new(r"").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "search",
                    &add_boundaries(SearchType::Pattern(re)),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_single_char() {
            let re = Regex::new(r"a").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "b a c",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "X"
                ),
                Some("b X c".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "bac",
                    &add_boundaries(SearchType::Pattern(re)),
                    "X"
                ),
                None
            );
        }

        #[test]
        fn test_escaped_chars() {
            let re = Regex::new(r"\(\d+\)").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "test (123) foo",
                    &add_boundaries(SearchType::Pattern(re)),
                    "X"
                ),
                Some("test X foo".to_string())
            );
        }

        #[test]
        fn test_with_unicode() {
            let re = Regex::new(r"λ\d+").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "calc λ123 β",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "X"
                ),
                Some("calc X β".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "calcλ123",
                    &add_boundaries(SearchType::Pattern(re)),
                    "X"
                ),
                None
            );
        }

        #[test]
        fn test_multiline_patterns() {
            let re = Regex::new(r"foo\s*\n\s*bar").unwrap();
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "test foo\nbar end",
                    &add_boundaries(SearchType::Pattern(re.clone())),
                    "NEW"
                ),
                Some("test NEW end".to_string())
            );
            assert_eq!(
                replace_whole_word_if_match_regex(
                    "test foo\n  bar end",
                    &add_boundaries(SearchType::Pattern(re)),
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
                replacement_if_match_any(
                    "foobarbaz",
                    &SearchType::Fixed("bar".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"bar").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "foobarbaz",
                    &SearchType::Fixed("xyz".to_string()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replacement_if_match_any(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"xyz").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "foo bar baz",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "embargo",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replacement_if_match_any(
                    "foo bar baz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "John Doe",
                    &SearchType::Pattern(Regex::new(r"(\w+)\s+(\w+)").unwrap()),
                    "$2, $1"
                ),
                Some("Doe, John".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "aaa123456bbb",
                    &SearchType::Pattern(Regex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "abc123def456",
                    &SearchType::Pattern(Regex::new(r"\d{3}").unwrap()),
                    "REPL"
                ),
                Some("abcREPLdefREPL".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "aaa123456bbb",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "foo.bar*baz",
                    &SearchType::Fixed(".bar*".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "foo.bar*baz",
                    &SearchType::Pattern(Regex::new(r"\.bar\*").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "Hello 世界!",
                    &SearchType::Fixed("世界".to_string()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "Hello 世界!",
                    &SearchType::Pattern(Regex::new(r"世界").unwrap()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
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
                replacement_if_match_any(
                    "HELLO world",
                    &SearchType::Pattern(Regex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
            assert_eq!(
                replacement_if_match_any(
                    "HELLO world",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
        }
    }
}
