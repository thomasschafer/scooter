use content_inspector::{inspect, ContentType};
use fancy_regex::Regex as FancyRegex;
use ignore::overrides::Override;
use ignore::{WalkBuilder, WalkParallel};
use log::warn;
use regex::Regex;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::thread;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::{fs::File, io::BufReader};

use crate::app::{BackgroundProcessingEvent, SearchFieldValues, SearchResult};

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

    let fancy_regex = FancyRegex::new(&search_regex_str).unwrap();
    SearchType::PatternAdvanced(fancy_regex)
}

#[derive(Clone, Debug)]
pub struct ParsedFields {
    search: SearchType,
    replace: String,
    overrides: Override,
    // TODO: `root_dir` and `include_hidden` are duplicated across this and App
    root_dir: PathBuf,
    include_hidden: bool,

    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
}

impl ParsedFields {
    // TODO: add tests for instantiating and handling paths
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search: SearchType,
        replace: String,
        whole_word: bool,
        match_case: bool,
        overrides: Override,
        root_dir: PathBuf,
        include_hidden: bool,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> Self {
        let search = if whole_word == SearchFieldValues::whole_word_default()
            && match_case == SearchFieldValues::match_case_default()
        {
            search
        } else {
            convert_regex(&search, whole_word, match_case)
        };
        Self {
            search,
            replace,
            overrides,
            root_dir,
            include_hidden,
            background_processing_sender,
        }
    }

    pub async fn handle_path(&self, path: &Path) {
        match File::open(path).await {
            Ok(file) => {
                let reader = BufReader::new(file);

                let mut lines = reader.lines();
                let mut line_number = 0;
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if let ContentType::BINARY = inspect(line.as_bytes()) {
                                break;
                            }
                            if let Some(replacement) =
                                replacement_if_match(&line, &self.search, &self.replace)
                            {
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
                            warn!("Error retrieving line {line_number} of {path:?}: {err}");
                        }
                    }
                    line_number += 1;
                }
            }
            Err(err) => {
                warn!("Error opening file {path:?}: {err}");
            }
        }
    }

    pub(crate) fn build_walker(&self) -> WalkParallel {
        // Default threads copied from ripgrep
        let threads = thread::available_parallelism()
            .map_or(1, NonZero::get)
            .min(12);
        WalkBuilder::new(&self.root_dir)
            .hidden(!self.include_hidden)
            .overrides(self.overrides.clone())
            .filter_entry(|entry| entry.file_name() != ".git")
            .threads(threads)
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
                replacement_if_match(
                    "hello world",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                Some("hello earth".to_string())
            );
        }

        #[test]
        fn test_multiple_replacements() {
            assert_eq!(
                replacement_if_match(
                    "world hello world",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                Some("earth hello earth".to_string())
            );
        }

        #[test]
        fn test_no_match() {
            assert_eq!(
                replacement_if_match(
                    "worldwide",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "_world_",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_word_boundaries() {
            assert_eq!(
                replacement_if_match(
                    ",world-",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                Some(",earth-".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "world-word",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                Some("earth-word".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "Hello-world!",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                Some("Hello-earth!".to_string())
            );
        }

        #[test]
        fn test_case_sensitive() {
            assert_eq!(
                replacement_if_match(
                    "Hello WORLD",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, true),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "Hello world",
                    &convert_regex(&SearchType::Fixed("wOrld".to_string()), true, true),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_empty_strings() {
            assert_eq!(
                replacement_if_match(
                    "",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "hello world",
                    &convert_regex(&SearchType::Fixed("".to_string()), true, false),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_substring_no_match() {
            assert_eq!(
                replacement_if_match(
                    "worldwide web",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "underworld",
                    &convert_regex(&SearchType::Fixed("world".to_string()), true, false),
                    "earth"
                ),
                None
            );
        }

        #[test]
        fn test_special_regex_chars() {
            assert_eq!(
                replacement_if_match(
                    "hello (world)",
                    &convert_regex(&SearchType::Fixed("(world)".to_string()), true, false),
                    "earth"
                ),
                Some("hello earth".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "hello world.*",
                    &convert_regex(&SearchType::Fixed("world.*".to_string()), true, false),
                    "ea+rth"
                ),
                Some("hello ea+rth".to_string())
            );
        }

        #[test]
        fn test_basic_regex_patterns() {
            let re = Regex::new(r"ax*b").unwrap();
            assert_eq!(
                replacement_if_match(
                    "foo axxxxb bar",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "NEW"
                ),
                Some("foo NEW bar".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "fooaxxxxb bar",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_patterns_with_spaces() {
            let re = Regex::new(r"hel+o world").unwrap();
            assert_eq!(
                replacement_if_match(
                    "say hello world!",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "hi earth"
                ),
                Some("say hi earth!".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "helloworld",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "hi earth"
                ),
                None
            );
        }

        #[test]
        fn test_multiple_matches() {
            let re = Regex::new(r"a+b+").unwrap();
            assert_eq!(
                replacement_if_match(
                    "foo aab abb",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("foo X X".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "ab abaab abb",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("X abaab X".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "ababaababb",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "ab ab aab abb",
                    &convert_regex(&SearchType::Pattern(re), true, false),
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
                replacement_if_match(
                    "foo bar baz",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "TEST"
                ),
                Some("TEST baz".to_string())
            );
            // At end of string
            assert_eq!(
                replacement_if_match(
                    "baz foo bar",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "TEST"
                ),
                Some("baz TEST".to_string())
            );
            // With punctuation
            assert_eq!(
                replacement_if_match(
                    "(foo bar)",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "TEST"
                ),
                Some("(TEST)".to_string())
            );
        }

        #[test]
        fn test_with_punctuation() {
            let re = Regex::new(r"a\d+b").unwrap();
            assert_eq!(
                replacement_if_match(
                    "(a123b)",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("(X)".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "foo.a123b!bar",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("foo.X!bar".to_string())
            );
        }

        #[test]
        fn test_complex_patterns() {
            let re = Regex::new(r"[a-z]+\d+[a-z]+").unwrap();
            assert_eq!(
                replacement_if_match(
                    "test9 abc123def 8xyz",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "NEW"
                ),
                Some("test9 NEW 8xyz".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "test9abc123def8xyz",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_optional_patterns() {
            let re = Regex::new(r"colou?r").unwrap();
            assert_eq!(
                replacement_if_match(
                    "my color and colour",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("my X and X".to_string())
            );
        }

        #[test]
        fn test_empty_haystack() {
            let re = Regex::new(r"test").unwrap();
            assert_eq!(
                replacement_if_match(
                    "",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_empty_search_regex() {
            let re = Regex::new(r"").unwrap();
            assert_eq!(
                replacement_if_match(
                    "search",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "NEW"
                ),
                None
            );
        }

        #[test]
        fn test_single_char() {
            let re = Regex::new(r"a").unwrap();
            assert_eq!(
                replacement_if_match(
                    "b a c",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("b X c".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "bac",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                None
            );
        }

        #[test]
        fn test_escaped_chars() {
            let re = Regex::new(r"\(\d+\)").unwrap();
            assert_eq!(
                replacement_if_match(
                    "test (123) foo",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                Some("test X foo".to_string())
            );
        }

        #[test]
        fn test_with_unicode() {
            let re = Regex::new(r"λ\d+").unwrap();
            assert_eq!(
                replacement_if_match(
                    "calc λ123 β",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "X"
                ),
                Some("calc X β".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "calcλ123",
                    &convert_regex(&SearchType::Pattern(re), true, false),
                    "X"
                ),
                None
            );
        }

        #[test]
        fn test_multiline_patterns() {
            let re = Regex::new(r"foo\s*\n\s*bar").unwrap();
            assert_eq!(
                replacement_if_match(
                    "test foo\nbar end",
                    &convert_regex(&SearchType::Pattern(re.clone()), true, false),
                    "NEW"
                ),
                Some("test NEW end".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "test foo\n  bar end",
                    &convert_regex(&SearchType::Pattern(re), true, false),
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
                replacement_if_match("foobarbaz", &SearchType::Fixed("bar".to_string()), "REPL"),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"bar").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match("foobarbaz", &SearchType::Fixed("xyz".to_string()), "REPL"),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"xyz").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match(
                    "foo bar baz",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "embargo",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replacement_if_match(
                    "foo bar baz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match(
                    "John Doe",
                    &SearchType::Pattern(Regex::new(r"(\w+)\s+(\w+)").unwrap()),
                    "$2, $1"
                ),
                Some("Doe, John".to_string())
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match(
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
                replacement_if_match(
                    "aaa123456bbb",
                    &SearchType::Pattern(Regex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "abc123def456",
                    &SearchType::Pattern(Regex::new(r"\d{3}").unwrap()),
                    "REPL"
                ),
                Some("abcREPLdefREPL".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "aaa123456bbb",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::Fixed(".bar*".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "foo.bar*baz",
                    &SearchType::Pattern(Regex::new(r"\.bar\*").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match(
                    "Hello 世界!",
                    &SearchType::Fixed("世界".to_string()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "Hello 世界!",
                    &SearchType::Pattern(Regex::new(r"世界").unwrap()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replacement_if_match(
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
                replacement_if_match(
                    "HELLO world",
                    &SearchType::Pattern(Regex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
            assert_eq!(
                replacement_if_match(
                    "HELLO world",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
        }
    }
}
