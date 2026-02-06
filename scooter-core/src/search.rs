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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Line {
    pub content: String,
    pub line_ending: LineEnding,
}

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

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LinePos {
    pub line: usize, // 1-indexed
    pub byte_pos: usize,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum MatchContent {
    /// Line-mode: Replace all occurrences of pattern on the line
    /// Used for non-multiline search where we replace ALL matches on a single line
    Line {
        line_number: usize,
        content: String,
        line_ending: LineEnding,
    },
    /// Byte-mode: Replace only the specific byte range
    /// Used for multiline search where we track individual matches precisely
    ByteRange {
        lines: Vec<(usize, Line)>, // Line numbers (1-indexed) and line contents
        match_start_in_first_line: usize, // Byte offset where match starts in first line
        match_end_in_last_line: usize, // Byte offset where match ends in last line (exclusive)
        byte_start: usize,         // Absolute byte position in file
        byte_end: usize,           // Absolute byte position in file (exclusive)
        content: String,           // The matched bytes
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    Line,
    ByteRange,
}

impl MatchContent {
    /// Returns the matched text without line ending
    pub fn matched_text(&self) -> &str {
        match self {
            MatchContent::Line { content, .. } | MatchContent::ByteRange { content, .. } => content,
        }
    }

    pub fn mode(&self) -> MatchMode {
        match self {
            MatchContent::Line { .. } => MatchMode::Line,
            MatchContent::ByteRange { .. } => MatchMode::ByteRange,
        }
    }
}

/// Asserts all results use the same `MatchContent` variant and returns the mode.
/// Returns `None` if results is empty.
pub fn match_mode_of_results(results: &[SearchResultWithReplacement]) -> Option<MatchMode> {
    let first = results.first()?;
    let mode = first.search_result.content.mode();
    assert!(
        results
            .iter()
            .all(|r| r.search_result.content.mode() == mode),
        "Inconsistent MatchContent variants detected in results"
    );
    Some(mode)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: Option<PathBuf>,
    pub content: MatchContent,
    /// Whether to replace the given match
    pub included: bool,
}

impl SearchResult {
    /// Creates a `SearchResult` with line-mode content (single line)
    pub fn new_line(
        path: Option<PathBuf>,
        line_number: usize,
        content: String,
        line_ending: LineEnding,
        included: bool,
    ) -> Self {
        Self {
            path,
            content: MatchContent::Line {
                line_number,
                content,
                line_ending,
            },
            included,
        }
    }

    /// Creates a `SearchResult` with byte-range content
    #[allow(clippy::too_many_arguments)]
    pub fn new_byte_range(
        path: Option<PathBuf>,
        lines: Vec<(usize, Line)>,
        match_start_in_first_line: usize,
        match_end_in_last_line: usize,
        byte_start: usize,
        byte_end: usize,
        content: String,
        included: bool,
    ) -> Self {
        assert!(!lines.is_empty(), "ByteRange must have at least one line");
        assert!(
            match_start_in_first_line <= lines[0].1.content.len(),
            "match_start_in_first_line ({}) exceeds first line length ({})",
            match_start_in_first_line,
            lines[0].1.content.len()
        );
        assert!(
            match_end_in_last_line <= lines.last().unwrap().1.content.len(),
            "match_end_in_last_line ({}) exceeds last line length ({})",
            match_end_in_last_line,
            lines.last().unwrap().1.content.len()
        );
        assert!(
            byte_start < byte_end,
            "byte_start ({byte_start}) must be < byte_end ({byte_end})",
        );

        for i in 1..lines.len() {
            assert!(
                lines[i].0 == lines[i - 1].0 + 1,
                "Line numbers must be sequential: {} followed by {}",
                lines[i - 1].0,
                lines[i].0
            );
        }

        Self {
            path,
            content: MatchContent::ByteRange {
                lines,
                match_start_in_first_line,
                match_end_in_last_line,
                byte_start,
                byte_end,
                content,
            },
            included,
        }
    }

    /// Returns the full content string for this match (including line ending for Lines mode)
    pub fn content_string(&self) -> String {
        match &self.content {
            MatchContent::Line {
                content,
                line_ending,
                ..
            } => format!("{}{}", content, line_ending.as_str()),
            MatchContent::ByteRange { content, .. } => content.clone(),
        }
    }

    /// Returns start line number
    pub fn start_line_number(&self) -> usize {
        match &self.content {
            MatchContent::Line { line_number, .. } => *line_number,
            MatchContent::ByteRange { lines, .. } => {
                lines
                    .first()
                    .expect("ByteRange must have at least one line")
                    .0
            }
        }
    }

    /// Returns end line number
    pub fn end_line_number(&self) -> usize {
        match &self.content {
            MatchContent::Line { line_number, .. } => *line_number,
            MatchContent::ByteRange { lines, .. } => {
                lines
                    .last()
                    .expect("ByteRange must have at least one line")
                    .0
            }
        }
    }
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
            self.search_result.start_line_number()
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

    pub fn multiline(&self) -> bool {
        self.search_config.multiline
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
    /// Whether to search and replace across multiple lines
    pub multiline: bool,
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
    ///     multiline: false,
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
                    let results = match search_file(
                        entry.path(),
                        &self.search_config.search,
                        self.search_config.multiline,
                    ) {
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
                    match replace::replace_all_in_file(
                        entry.path(),
                        self.search(),
                        self.replace(),
                        self.multiline(),
                    ) {
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

pub fn contains_search(haystack: &str, needle: &SearchType) -> bool {
    match needle {
        SearchType::Fixed(fixed_str) => haystack.contains(fixed_str),
        SearchType::Pattern(pattern) => pattern.is_match(haystack),
        SearchType::PatternAdvanced(pattern) => pattern.is_match(haystack).is_ok_and(|r| r),
    }
}

pub fn search_file(
    path: &Path,
    search: &SearchType,
    multiline: bool,
) -> anyhow::Result<Vec<SearchResult>> {
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

    if multiline {
        let content = std::fs::read_to_string(path)?;
        return Ok(search_multiline(&content, search, Some(path)));
    }

    // Line-by-line search for non-multiline mode
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

        if let Ok(line_content) = String::from_utf8(line_bytes)
            && contains_search(&line_content, search)
        {
            let result = SearchResult::new_line(
                Some(path.to_path_buf()),
                line_number,
                line_content,
                line_ending,
                true,
            );
            results.push(result);
        }
    }

    Ok(results)
}

/// Search content for multiline patterns and return `SearchResults`
pub(crate) fn search_multiline(
    content: &str,
    search: &SearchType,
    path: Option<&Path>,
) -> Vec<SearchResult> {
    // Pre-compute newline positions for efficient line number lookups
    let line_index = LineIndex::new(content);

    let matches: Box<dyn Iterator<Item = (usize, usize)>> = match search {
        SearchType::Fixed(pattern) => Box::new(
            content
                .match_indices(pattern.as_str())
                .map(|(byte_offset, _)| (byte_offset, byte_offset + pattern.len())),
        ),
        SearchType::Pattern(regex) => {
            Box::new(regex.find_iter(content).map(|mat| (mat.start(), mat.end())))
        }
        SearchType::PatternAdvanced(regex) => Box::new(
            regex
                .find_iter(content)
                .flatten()
                .map(|mat| (mat.start(), mat.end())),
        ),
    };

    matches
        .map(|(start, end)| {
            // `end` is exclusive so should always be greater than `start`
            assert!(
                start < end,
                "Found match with start >= end: start = {start}, end = {end}",
            );
            create_search_result_from_bytes(start, end, path, &line_index)
        })
        .collect()
}

/// Helper struct to efficiently convert byte offsets to line numbers and extract lines
pub(crate) struct LineIndex<'a> {
    content: &'a str,
    /// Byte positions of newline characters
    newline_positions: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(content: &'a str) -> Self {
        let newline_positions: Vec<usize> = content
            .char_indices()
            .filter_map(|(i, c)| if c == '\n' { Some(i) } else { None })
            .collect();
        Self {
            content,
            newline_positions,
        }
    }

    /// Get line number (1-indexed) for a byte offset
    pub(crate) fn line_number_at(&self, byte_offset: usize) -> usize {
        // Binary search to find how many newlines come before this offset
        // Both Ok and Err return the same value: the number of newlines before/at this position + 1
        match self.newline_positions.binary_search(&byte_offset) {
            Ok(idx) | Err(idx) => idx + 1,
        }
    }

    /// Get the byte offset where a line starts (`line_num` is 1-indexed)
    pub(crate) fn line_start_byte(&self, line_num: usize) -> usize {
        assert!(line_num >= 1, "Line numbers are 1-indexed");
        if line_num == 1 {
            0
        } else {
            // Line N starts after the (N-1)th newline
            self.newline_positions[line_num - 2] + 1
        }
    }

    /// Get the byte offset where a line ends (exclusive, i.e., points to newline or end of content)
    fn line_end_byte(&self, line_num: usize) -> usize {
        assert!(line_num >= 1, "Line numbers are 1-indexed");
        // The end of line N is at the N-1 index in newline_positions (0-indexed)
        if line_num <= self.newline_positions.len() {
            self.newline_positions[line_num - 1]
        } else {
            // Last line without trailing newline
            self.content.len()
        }
    }

    /// Returns the total number of lines in the content
    fn total_lines(&self) -> usize {
        // Number of newlines + 1, unless the file is empty
        if self.content.is_empty() {
            0
        } else {
            self.newline_positions.len() + 1
        }
    }

    /// Extract full lines from `start_line` to `end_line` (both 1-indexed, inclusive)
    pub(crate) fn extract_lines(&self, start_line: usize, end_line: usize) -> Vec<(usize, Line)> {
        assert!(start_line >= 1, "Line numbers are 1-indexed");
        assert!(start_line <= end_line, "start_line must be <= end_line");

        (start_line..=end_line)
            .map(|line_num| {
                let start = self.line_start_byte(line_num);
                let end = self.line_end_byte(line_num);
                let content = self.content[start..end].to_string();

                // Determine line ending
                let line_ending = if line_num <= self.newline_positions.len() {
                    let newline_pos = self.newline_positions[line_num - 1];
                    if newline_pos > 0
                        && self.content.as_bytes().get(newline_pos.saturating_sub(1))
                            == Some(&b'\r')
                    {
                        LineEnding::CrLf
                    } else {
                        LineEnding::Lf
                    }
                } else {
                    LineEnding::None
                };

                (
                    line_num,
                    Line {
                        content,
                        line_ending,
                    },
                )
            })
            .collect()
    }
}

/// Create a `SearchResult` from byte offsets in the content.
/// `end_byte` is exclusive (standard Rust range semantics).
fn create_search_result_from_bytes(
    start_byte: usize,
    end_byte: usize,
    path: Option<&Path>,
    line_index: &LineIndex<'_>,
) -> SearchResult {
    debug_assert!(
        start_byte < end_byte,
        "Zero-length matches are not supported: start_byte={start_byte}, end_byte={end_byte}"
    );

    let start_line_num = line_index.line_number_at(start_byte);
    // end_byte is exclusive, so we use end_byte - 1 for the line number
    let mut end_line_num = line_index.line_number_at(end_byte.saturating_sub(1));

    // Compute byte offsets within each line (for preview highlighting)
    let match_start_in_first_line = start_byte - line_index.line_start_byte(start_line_num);

    let last_line_start = line_index.line_start_byte(end_line_num);
    let last_line_end = line_index.line_end_byte(end_line_num);
    let last_line_content_len = last_line_end - last_line_start;

    // Check if match extends into the line ending (newline)
    // If so, and there's a next line, include it so the preview shows the merge
    let match_end_in_last_line = if end_byte > last_line_start + last_line_content_len {
        // Match extends past line content into line ending
        let has_next_line = end_line_num < line_index.total_lines();
        if has_next_line {
            // Include next line with match_end = 0 (match doesn't extend into its content)
            end_line_num += 1;
            0
        } else {
            last_line_content_len
        }
    } else {
        end_byte - last_line_start
    };

    // Extract full lines containing the match
    let lines = line_index.extract_lines(start_line_num, end_line_num);

    // Extract the matched content
    let expected_content = line_index.content[start_byte..end_byte].to_string();

    SearchResult::new_byte_range(
        path.map(Path::to_path_buf),
        lines,
        match_start_in_first_line,
        match_end_in_last_line,
        start_byte,
        end_byte,
        expected_content,
        true,
    )
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
                search_result: SearchResult::new_line(
                    Some(PathBuf::from(path)),
                    line_number,
                    "test line".to_string(),
                    LineEnding::Lf,
                    true,
                ),
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

            let result = replace::replace_all_if_match(text, &search, "World");

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
                replace::replace_all_if_match(text, &search, "e"),
                Some("cafe".to_string())
            );
        }

        #[test]
        fn test_unicode_regex_classes() {
            let text = "Latin A, Cyrillic Б, Greek Γ, Hebrew א";

            let search = SearchType::Pattern(Regex::new(r"\p{Cyrillic}").unwrap());
            assert_eq!(
                replace::replace_all_if_match(text, &search, "X"),
                Some("Latin A, Cyrillic X, Greek Γ, Hebrew א".to_string())
            );

            let search = SearchType::Pattern(Regex::new(r"\p{Greek}").unwrap());
            assert_eq!(
                replace::replace_all_if_match(text, &search, "X"),
                Some("Latin A, Cyrillic Б, Greek X, Hebrew א".to_string())
            );
        }

        #[test]
        fn test_unicode_capture_groups() {
            let text = "Name: 李明 (ID: A12345)";

            let search =
                SearchType::Pattern(Regex::new(r"Name: (\p{Han}+) \(ID: ([A-Z0-9]+)\)").unwrap());
            assert_eq!(
                replace::replace_all_if_match(text, &search, "ID $2 belongs to $1"),
                Some("ID A12345 belongs to 李明".to_string())
            );
        }
    }

    mod replace_any {
        use super::*;

        #[test]
        fn test_simple_match_subword() {
            assert_eq!(
                replace::replace_all_if_match(
                    "foobarbaz",
                    &SearchType::Fixed("bar".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"bar").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "foobarbaz",
                    &SearchType::Fixed("xyz".to_string()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "foobarbaz",
                    &SearchType::Pattern(Regex::new(r"xyz").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "foo bar baz",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "embargo",
                    &SearchType::Pattern(Regex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                None
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "foo bar baz",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\bbar\b").unwrap()),
                    "REPL"
                ),
                Some("foo REPL baz".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "John Doe",
                    &SearchType::Pattern(Regex::new(r"(\w+)\s+(\w+)").unwrap()),
                    "$2, $1"
                ),
                Some("Doe, John".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "aaa123456bbb",
                    &SearchType::Pattern(Regex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "abc123def456",
                    &SearchType::Pattern(Regex::new(r"\d{3}").unwrap()),
                    "REPL"
                ),
                Some("abcREPLdefREPL".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "aaa123456bbb",
                    &SearchType::PatternAdvanced(FancyRegex::new(r"\d+").unwrap()),
                    "REPL"
                ),
                Some("aaaREPLbbb".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "foo.bar*baz",
                    &SearchType::Fixed(".bar*".to_string()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "foo.bar*baz",
                    &SearchType::Pattern(Regex::new(r"\.bar\*").unwrap()),
                    "REPL"
                ),
                Some("fooREPLbaz".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "Hello 世界!",
                    &SearchType::Fixed("世界".to_string()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
                    "Hello 世界!",
                    &SearchType::Pattern(Regex::new(r"世界").unwrap()),
                    "REPL"
                ),
                Some("Hello REPL!".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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
                replace::replace_all_if_match(
                    "HELLO world",
                    &SearchType::Pattern(Regex::new(r"(?i)hello").unwrap()),
                    "REPL"
                ),
                Some("REPL world".to_string())
            );
            assert_eq!(
                replace::replace_all_if_match(
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

    mod multiline_tests {
        use super::*;

        #[test]
        fn test_line_index_single_line() {
            let content = "single line";
            let index = LineIndex::new(content);
            assert_eq!(index.line_number_at(0), 1);
            assert_eq!(index.line_number_at(6), 1);
            assert_eq!(index.line_number_at(11), 1);
        }

        #[test]
        fn test_line_index_multiple_lines() {
            let content = "line 1\nline 2\nline 3";
            let index = LineIndex::new(content);

            // Line 1 (bytes 0-5)
            assert_eq!(index.line_number_at(0), 1);
            assert_eq!(index.line_number_at(5), 1);

            // Newline at byte 6
            assert_eq!(index.line_number_at(6), 1);

            // Line 2 (bytes 7-12)
            assert_eq!(index.line_number_at(7), 2);
            assert_eq!(index.line_number_at(12), 2);

            // Newline at byte 13
            assert_eq!(index.line_number_at(13), 2);

            // Line 3 (bytes 14-19)
            assert_eq!(index.line_number_at(14), 3);
            assert_eq!(index.line_number_at(19), 3);
        }

        #[test]
        fn test_line_index_empty_lines() {
            let content = "line 1\n\nline 3";
            let index = LineIndex::new(content);

            assert_eq!(index.line_number_at(0), 1); // "l" in line 1
            assert_eq!(index.line_number_at(6), 1); // first newline
            assert_eq!(index.line_number_at(7), 2); // second newline (empty line)
            assert_eq!(index.line_number_at(8), 3); // "l" in line 3
        }

        #[test]
        fn test_search_multiline_fixed_string() {
            let content = "foo\nbar\nbaz";
            let search = SearchType::Fixed("foo\nb".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].start_line_number(), 1);
            assert_eq!(results[0].end_line_number(), 2);
            assert_eq!(results[0].path, None);
            let MatchContent::ByteRange {
                content: expected_content,
                ..
            } = &results[0].content
            else {
                panic!("Expected ByteRange content");
            };
            assert_eq!(expected_content, "foo\nb");
        }

        #[test]
        fn test_search_multiline_regex_pattern() {
            let content = "start\nmiddle\nend\nother";
            let search = SearchType::Pattern(regex::Regex::new(r"start.*\nmiddle").unwrap());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].start_line_number(), 1);
            assert_eq!(results[0].end_line_number(), 2);
            let MatchContent::ByteRange {
                content: expected_content,
                ..
            } = &results[0].content
            else {
                panic!("Expected ByteRange content");
            };
            assert_eq!(expected_content, "start\nmiddle");
        }

        #[test]
        fn test_search_multiline_multiple_matches() {
            let content = "foo\nbar\n\nfoo\nbar\nbaz";
            let search = SearchType::Fixed("foo\nb".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 2);
            assert_eq!(results[0].start_line_number(), 1);
            assert_eq!(results[0].end_line_number(), 2);
            assert_eq!(results[1].start_line_number(), 4);
            assert_eq!(results[1].end_line_number(), 5);
        }

        #[test]
        fn test_search_multiline_no_matches() {
            let content = "foo\nbar\nbaz";
            let search = SearchType::Fixed("not_found".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 0);
        }

        #[test]
        fn test_search_multiline_with_path() {
            let content = "test\ndata";
            let path = Path::new("/test/file.txt");
            let search = SearchType::Fixed("test".to_string());
            let results = search_multiline(content, &search, Some(path));

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].path, Some(PathBuf::from("/test/file.txt")));
        }

        #[test]
        fn test_search_multiline_line_endings_crlf() {
            let content = "foo\r\nbar";
            let search = SearchType::Fixed("foo\r\n".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            let MatchContent::ByteRange {
                content: expected_content,
                ..
            } = &results[0].content
            else {
                panic!("Expected ByteRange");
            };
            assert_eq!(expected_content, "foo\r\n");
        }

        #[test]
        fn test_search_multiline_line_endings_lf() {
            let content = "foo\nbar";
            let search = SearchType::Fixed("foo\n".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            let MatchContent::ByteRange {
                content: expected_content,
                ..
            } = &results[0].content
            else {
                panic!("Expected ByteRange");
            };
            assert_eq!(expected_content, "foo\n");
        }

        #[test]
        fn test_search_multiline_line_endings_none() {
            let content = "foobar";
            let search = SearchType::Fixed("foo".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            let MatchContent::ByteRange {
                content: expected_content,
                ..
            } = &results[0].content
            else {
                panic!("Expected ByteRange");
            };
            // Only "foo" is matched, not the full line
            assert_eq!(expected_content, "foo");
        }

        #[test]
        fn test_search_multiline_spanning_three_lines() {
            let content = "line1\nline2\nline3\nline4";
            let search = SearchType::Fixed("ne1\nline2\nli".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].start_line_number(), 1);
            assert_eq!(results[0].end_line_number(), 3);
            assert_eq!(results[0].path, None);
            assert_eq!(results[0].included, true);
            let MatchContent::ByteRange {
                content: expected_content,
                ..
            } = &results[0].content
            else {
                panic!("Expected ByteRange");
            };
            assert_eq!(expected_content, "ne1\nline2\nli");
        }

        #[test]
        fn test_search_multiline_pattern_at_end() {
            let content = "start\npattern\nend";
            let search = SearchType::Fixed("pattern\nend".to_string());
            let results = search_multiline(content, &search, None);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].start_line_number(), 2);
            assert_eq!(results[0].end_line_number(), 3);
        }

        #[test]
        fn test_create_search_result_single_line_match() {
            let content = "line1\nline2\nline3";
            let line_index = LineIndex::new(content);

            let result = create_search_result_from_bytes(6, 11, None, &line_index);

            assert_eq!(result.start_line_number(), 2);
            assert_eq!(result.end_line_number(), 2);
            let MatchContent::ByteRange {
                content: expected_content,
                byte_start,
                byte_end,
                ..
            } = &result.content
            else {
                panic!("Expected ByteRange");
            };
            assert_eq!(expected_content, "line2");
            assert_eq!((*byte_start, *byte_end), (6, 11));
        }

        #[test]
        fn test_create_search_result_multiline_match() {
            let content = "line1\nline2\nline3";
            let line_index = LineIndex::new(content);

            let result = create_search_result_from_bytes(0, 11, None, &line_index);

            assert_eq!(result.start_line_number(), 1);
            assert_eq!(result.end_line_number(), 2);
            let MatchContent::ByteRange {
                content: expected_content,
                byte_start,
                byte_end,
                ..
            } = &result.content
            else {
                panic!("Expected ByteRange");
            };
            assert_eq!(expected_content, "line1\nline2");
            assert_eq!((*byte_start, *byte_end), (0, 11));
        }
    }

    #[test]
    fn test_multiple_matches_per_line() {
        // "foo\nbar baz bar qux\nbar\nbux\n"
        //  0123 456789012345678 901234567
        //       ^     ^         ^
        //       4-7   12-15     20-23  (exclusive end)
        let content = "foo\nbar baz bar qux\nbar\nbux\n";
        let search = SearchType::Fixed("bar".to_string());

        let results = search_multiline(content, &search, None);

        // Should find 3 matches: 2 on line 2, 1 on line 3
        assert_eq!(results.len(), 3);

        // First match: "bar" at bytes 4-7 on line 2
        assert_eq!(results[0].start_line_number(), 2);
        assert_eq!(results[0].end_line_number(), 2);
        let MatchContent::ByteRange {
            byte_start: byte_start_0,
            byte_end: byte_end_0,
            ..
        } = &results[0].content
        else {
            panic!("Expected ByteRange");
        };
        assert_eq!((*byte_start_0, *byte_end_0), (4, 7));

        // Second match: "bar" at bytes 12-15 on line 2 (same line!)
        assert_eq!(results[1].start_line_number(), 2);
        assert_eq!(results[1].end_line_number(), 2);
        let MatchContent::ByteRange {
            byte_start: byte_start_1,
            byte_end: byte_end_1,
            ..
        } = &results[1].content
        else {
            panic!("Expected ByteRange");
        };
        assert_eq!((*byte_start_1, *byte_end_1), (12, 15));

        // Third match: "bar" at bytes 20-23 on line 3
        assert_eq!(results[2].start_line_number(), 3);
        assert_eq!(results[2].end_line_number(), 3);
        let MatchContent::ByteRange {
            byte_start: byte_start_2,
            byte_end: byte_end_2,
            ..
        } = &results[2].content
        else {
            panic!("Expected ByteRange");
        };
        assert_eq!((*byte_start_2, *byte_end_2), (20, 23));
    }
}
