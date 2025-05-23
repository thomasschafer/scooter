use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use content_inspector::{inspect, ContentType};
use crossbeam_channel::bounded;
use fancy_regex::Regex as FancyRegex;
use ignore::overrides::Override;
use ignore::{WalkBuilder, WalkState};
use log::{error, warn};
use regex::Regex;
use tokio::sync::mpsc::UnboundedSender;

use crate::{app::BackgroundProcessingEvent, fields::SearchFieldValues, replace::ReplaceResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_number: usize,
    /// 1-indexed
    pub line: String,
    pub replacement: String,
    pub included: bool,
    pub replace_result: Option<ReplaceResult>,
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

    // Shouldn't fail as we have already verified that `search` is valid, so `unwrap` here is fine.
    // (Any issues will likely be with the padding we are doing in this function.)
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
    cancelled: Arc<AtomicBool>,

    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
}

impl ParsedFields {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search: SearchType,
        replace: String,
        whole_word: bool,
        match_case: bool,
        overrides: Override,
        root_dir: PathBuf,
        include_hidden: bool,
        cancelled: Arc<AtomicBool>,
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
            cancelled,
            background_processing_sender,
        }
    }

    pub fn search_parallel(&self) {
        self.cancelled.store(false, Ordering::Relaxed);

        let (path_tx, path_rx) = bounded::<PathBuf>(1000);

        let num_threads = thread::available_parallelism()
            .map(NonZero::get)
            .unwrap_or(4)
            .min(12);

        let mut handles = vec![];

        for _ in 0..num_threads {
            let path_rx = path_rx.clone();
            let sender = self.background_processing_sender.clone();
            let search = self.search.clone();
            let replace = self.replace.clone();
            let cancelled = self.cancelled.clone();

            let handle = thread::spawn(move || {
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    match path_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(path) => {
                            Self::search_file(&path, &search, &replace, &sender);
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            // Keep polling
                        }
                        Err(_) => {
                            // Channel closed, exit
                            break;
                        }
                    }
                }
            });

            handles.push(handle);
        }

        let walker_handle = {
            let root_dir = self.root_dir.clone();
            let include_hidden = self.include_hidden;
            let overrides = self.overrides.clone();
            let path_tx = path_tx.clone();
            let cancelled = self.cancelled.clone();

            thread::spawn(move || {
                let walker = WalkBuilder::new(&root_dir)
                    .hidden(!include_hidden)
                    .overrides(overrides)
                    .filter_entry(|entry| entry.file_name() != ".git")
                    .threads(num_threads)
                    .build_parallel();

                walker.run(|| {
                    let path_tx = path_tx.clone();
                    let cancelled = cancelled.clone();
                    Box::new(move |result| {
                        if cancelled.load(Ordering::Relaxed) {
                            return WalkState::Quit;
                        }

                        let Ok(entry) = result else {
                            return WalkState::Continue;
                        };

                        if entry.file_type().is_some_and(|ft| ft.is_file())
                            && !Self::is_likely_binary(entry.path())
                            && path_tx.send(entry.path().to_owned()).is_err()
                        {
                            return WalkState::Quit;
                        }
                        WalkState::Continue
                    })
                });
            })
        };

        while !walker_handle.is_finished() {
            if self.cancelled.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        drop(path_tx);

        if !self.cancelled.load(Ordering::Relaxed) {
            let _ = walker_handle.join();

            for handle in handles {
                let _ = handle.join();
            }
        }
    }

    fn search_file(
        path: &Path,
        search: &SearchType,
        replace: &str,
        sender: &UnboundedSender<BackgroundProcessingEvent>,
    ) {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(err) => {
                warn!("Error opening file {path:?}: {err}");
                return;
            }
        };

        // Fast upfront binary sniff (8 KiB)
        let mut probe = [0u8; 8192];
        let read = file.read(&mut probe).unwrap_or(0);
        if matches!(inspect(&probe[..read]), ContentType::BINARY) {
            return;
        }
        if file.seek(SeekFrom::Start(0)).is_err() {
            error!("Failed to seek file {path:?} to start");
            return;
        }

        let reader = BufReader::with_capacity(16384, file);
        let mut results = Vec::new();

        let mut read_errors = 0;

        for (mut line_number, line_result) in reader.lines().enumerate() {
            line_number += 1; // Ensure line-number is 1-indexed

            let line = match line_result {
                Ok(l) => l,
                Err(err) => {
                    read_errors += 1;
                    warn!("Error retrieving line {line_number} of {path:?}: {err}");
                    if read_errors >= 10 {
                        break;
                    }
                    continue;
                }
            };

            if let Some(replacement) = replacement_if_match(&line, search, replace) {
                let result = SearchResult {
                    path: path.to_path_buf(),
                    line_number,
                    line,
                    replacement,
                    included: true,
                    replace_result: None,
                };
                results.push(result);
            }
        }

        if !results.is_empty() {
            // Ignore error - likely state reset, thread about to be killed
            let _ = sender.send(BackgroundProcessingEvent::AddSearchResults(results));
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
