use std::{
    fs::File,
    io::{self, BufReader},
    num::NonZeroUsize,
    ops::{Add, Div, Mul, Rem},
    path::Path,
};

use anyhow::{Context, Error, bail};
use ignore::overrides::OverrideBuilder;
use two_face::re_exports::syntect::{
    easy::HighlightLines,
    highlighting::{Style, Theme},
    parsing::SyntaxSet,
};

use crate::line_reader::{BufReadExt, LinesSplitEndings};

pub fn relative_path(base: &Path, target: &Path) -> String {
    match target.strip_prefix(base) {
        Ok(relative) => {
            // Successfully stripped - base is an ancestor
            if relative.as_os_str().is_empty() {
                if target.is_file() {
                    target
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(".")
                        .to_string()
                } else {
                    ".".to_string()
                }
            } else {
                relative.to_string_lossy().to_string()
            }
        }
        Err(_) => {
            // Base is not an ancestor - return full target path
            target.to_string_lossy().to_string()
        }
    }
}
pub fn group_by<I, T, F>(iter: I, predicate: F) -> Vec<Vec<T>>
where
    I: IntoIterator<Item = T>,
    F: Fn(&T, &T) -> bool,
{
    let mut result = Vec::new();
    let mut current_group = Vec::new();

    for item in iter {
        if current_group.is_empty() || predicate(current_group.last().unwrap(), &item) {
            current_group.push(item);
        } else {
            result.push(std::mem::take(&mut current_group));
            current_group.push(item);
        }
    }

    if !current_group.is_empty() {
        result.push(current_group);
    }

    result
}

pub fn surrounding_line_window<R>(
    reader: R,
    start: usize,
    end: usize,
) -> impl Iterator<Item = (usize, String)>
where
    R: BufReadExt,
{
    assert!(
        start <= end,
        "Expected start <= end, found start={start}, end={end}"
    );

    reader
        .lines_with_endings()
        .enumerate()
        .skip(start)
        .take(end - start + 1)
        .map(move |(idx, line_result)| {
            let line = match line_result {
                Ok((content, _ending)) => String::from_utf8_lossy(&content).into_owned(),
                Err(e) => {
                    log::error!("Error reading line {idx}: {e}");
                    String::new()
                }
            };
            (idx, line)
        })
}

pub fn read_lines_range(
    path: &Path,
    start: usize,
    end: usize,
) -> io::Result<impl Iterator<Item = (usize, String)>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    Ok(surrounding_line_window(reader, start, end))
}

/// Returns the largest range centred on `centre` that is both within `min_bound` and `max_bound`,
/// and is no larger than `max_size`.
///
/// # Example
/// ```
/// use std::num::NonZeroUsize;
/// use scooter_core::utils::largest_range_centered_on;
///
/// // Simple case - centered range with room on both sides
/// let (start, end) = largest_range_centered_on(5, 0, 10, NonZeroUsize::new(5).unwrap()).unwrap();
/// assert_eq!((3, 7), (start, end));
///
/// // Range limited by lower bound
/// let (start, end) = largest_range_centered_on(0, 0, 10, NonZeroUsize::new(5).unwrap()).unwrap();
/// assert_eq!((0, 4), (start, end));
///
/// // Range limited by upper bound
/// let (start, end) = largest_range_centered_on(8, 0, 10, NonZeroUsize::new(5).unwrap()).unwrap();
/// assert_eq!((6, 10), (start, end));
///
/// // Range limited by max_size
/// let (start, end) = largest_range_centered_on(5, 0, 20, NonZeroUsize::new(3).unwrap()).unwrap();
/// assert_eq!((4, 6), (start, end));
/// ```
pub fn largest_range_centered_on(
    centre: usize,
    lower_bound: usize,
    upper_bound: usize,
    max_size: NonZeroUsize,
) -> anyhow::Result<(usize, usize)> {
    if !(lower_bound <= centre && centre <= upper_bound) {
        bail!(
            "Expected start<=pos<=end, found start={lower_bound}, pos={centre}, end={upper_bound}",
        );
    }
    let max_size = max_size.get();

    let mut cur_size = 1;
    let mut cur_start = centre;
    let mut cur_end = centre;
    while cur_size < max_size && (cur_start > lower_bound || cur_end < upper_bound) {
        if cur_end < upper_bound {
            cur_end += 1;
            cur_size += 1;
        }
        if cur_size < max_size && cur_start > lower_bound {
            cur_start -= 1;
            cur_size += 1;
        }
    }

    Ok((cur_start, cur_end))
}

#[allow(clippy::type_complexity)]
pub fn split_indexed_lines<T>(
    indexed_lines: Vec<(usize, T)>,
    line_idx: usize,
    num_lines_to_show: u16,
) -> anyhow::Result<(Vec<(usize, T)>, (usize, T), Vec<(usize, T)>)> {
    let file_start = indexed_lines.first().context("No lines found")?.0;
    let file_end = indexed_lines.last().context("No lines found")?.0;
    let (new_start, new_end) = largest_range_centered_on(
        line_idx,
        file_start,
        file_end,
        NonZeroUsize::new(num_lines_to_show as usize).context("preview will have height 0")?,
    )?;

    let mut filtered_lines = indexed_lines
        .into_iter()
        .skip_while(|(idx, _)| *idx < new_start)
        .take_while(|(idx, _)| *idx <= new_end)
        .collect::<Vec<_>>();

    let position = filtered_lines
        .iter()
        .position(|(idx, _)| *idx == line_idx)
        .context("Couldn't find line in file")?;
    let after = filtered_lines.split_off(position + 1);
    let current = filtered_lines.pop().unwrap();

    Ok((filtered_lines, current, after))
}

pub fn strip_control_chars(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' => String::from("  "),
            '\n' => String::from(" "),
            c if c.is_control() => String::from("�"),
            c => String::from(c),
        })
        .collect()
}

pub fn ceil_div<T>(a: T, b: T) -> T
where
    T: Add<Output = T>
        + Div<Output = T>
        + Mul<Output = T>
        + Rem<Output = T>
        + From<bool>
        + From<u8>
        + PartialEq
        + PartialOrd
        + Copy,
{
    // If a * b <= 0 then division already rounds towards 0 i.e. up
    a / b + T::from((a * b > T::from(0)) && (a % b) != T::from(0))
}

pub type HighlightedLine = Vec<(Option<Style>, String)>;

struct HighlightedLinesIterator<'a> {
    lines: LinesSplitEndings<BufReader<File>>,
    highlighter: HighlightLines<'a>,
    syntax_set: &'a SyntaxSet,
    current_idx: usize,
    start_idx: usize,
    end_idx: Option<usize>,
    full_highlighting: bool,
}

impl<'a> HighlightedLinesIterator<'a> {
    pub fn new(
        path: &Path,
        theme: &'a Theme,
        syntax_set: &'a SyntaxSet,
        start_idx: Option<usize>,
        end_idx: Option<usize>,
        full_highlighting: bool,
    ) -> io::Result<Self> {
        let start_idx = start_idx.unwrap_or(0);
        if let Some(end_idx) = end_idx {
            #[allow(clippy::manual_assert)]
            if start_idx > end_idx {
                panic!("Expected start <= end, found start={start_idx}, end={end_idx}");
            }
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines_with_endings();

        // Skip lines if we're not doing full highlighting
        if !full_highlighting {
            for _ in 0..start_idx {
                if lines.next().is_none() {
                    break;
                }
            }
        }

        let syntax = syntax_set
            .find_syntax_for_file(path)?
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        Ok(Self {
            lines,
            highlighter: HighlightLines::new(syntax, theme),
            syntax_set,
            current_idx: if full_highlighting { 0 } else { start_idx },
            start_idx,
            end_idx,
            full_highlighting,
        })
    }
}

impl Iterator for HighlightedLinesIterator<'_> {
    type Item = (usize, HighlightedLine);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(end) = self.end_idx
            && self.current_idx > end
        {
            return None;
        }

        loop {
            let idx = self.current_idx;
            self.current_idx += 1;

            debug_assert!(
                self.full_highlighting || idx >= self.start_idx,
                "Should have skipped early lines before iteration"
            );

            match self.lines.next() {
                Some(Ok((content, _ending))) => {
                    // Convert to UTF-8 lossy, which replaces invalid sequences with the � character
                    let line = String::from_utf8_lossy(&content).into_owned();

                    let highlighted_res = self.highlighter.highlight_line(&line, self.syntax_set);
                    if let Err(ref e) = highlighted_res {
                        log::error!("Highlighting error at line {idx}: {e}");
                    }

                    if idx < self.start_idx {
                        continue;
                    }

                    let highlighted = match highlighted_res {
                        Ok(line) => line
                            .into_iter()
                            .map(|(style, text)| (Some(style), text.to_owned()))
                            .collect(),
                        Err(_) => {
                            vec![(None, line)]
                        }
                    };
                    return Some((idx, highlighted));
                }
                Some(Err(e)) => {
                    log::error!("Error reading line {}: {e}", self.current_idx);
                    return None;
                }
                None => return None, // EOF
            }
        }
    }
}

pub fn read_lines_range_highlighted<'a>(
    path: &'a Path,
    start: Option<usize>,
    end: Option<usize>,
    theme: &'a Theme,
    syntax_set: &'a SyntaxSet,
    full_highlighting: bool,
) -> io::Result<impl Iterator<Item = (usize, HighlightedLine)> + 'a> {
    HighlightedLinesIterator::new(path, theme, syntax_set, start, end, full_highlighting)
}

#[allow(dead_code)]
pub fn split_while<T, F>(vec: &[T], predicate: F) -> (&[T], &[T])
where
    F: Fn(&T) -> bool,
{
    match vec.iter().position(|x| !predicate(x)) {
        Some(index) => vec.split_at(index),
        None => (vec, &[]),
    }
}

pub fn last_n<T>(vec: &[T], n: usize) -> &[T] {
    &vec[vec.len().saturating_sub(n)..]
}

pub fn last_n_chars(s: &str, n: usize) -> &str {
    if n == 0 || s.is_empty() {
        return "";
    }
    let char_count = s.chars().count();
    if n >= char_count {
        return s;
    }

    let (idx, _) = s.char_indices().rev().nth(n - 1).unwrap();
    &s[idx..]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Either<T, S> {
    Left(T),
    Right(S),
}

pub fn is_regex_error(e: &Error) -> bool {
    e.downcast_ref::<regex::Error>().is_some() || e.downcast_ref::<fancy_regex::Error>().is_some()
}

pub fn add_overrides(
    overrides: &mut OverrideBuilder,
    files: &str,
    prefix: &str,
) -> anyhow::Result<()> {
    for file in files.split(',') {
        let file = file.trim();
        if !file.is_empty() {
            overrides.add(&format!("{prefix}{file}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use tempfile::NamedTempFile;
    use two_face::re_exports::syntect::highlighting::ThemeSet;

    use super::*;

    fn create_test_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    #[test]
    fn test_relative_path_file_in_dir() {
        // Directory to file in that directory
        assert_eq!(
            relative_path(Path::new("/foo/bar/"), Path::new("/foo/bar/baz.rs")),
            "baz.rs"
        );
        assert_eq!(
            relative_path(Path::new("/foo/bar"), Path::new("/foo/bar/baz.rs")),
            "baz.rs"
        );
    }

    #[test]
    fn test_relative_path_same_file() {
        // Same file to itself returns filename
        // We need to create the file so that `relative_path` can determine that it is indeed a file
        let test_file = create_test_file("");
        assert_eq!(
            relative_path(test_file.path(), test_file.path()),
            test_file.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn test_relative_path_same_dir() {
        // Same directory to itself returns "."
        assert_eq!(
            relative_path(Path::new("/foo/bar/"), Path::new("/foo/bar/")),
            "."
        );
    }

    #[test]
    fn test_relative_path_nested() {
        // Directory to nested file/directory
        assert_eq!(
            relative_path(Path::new("/foo"), Path::new("/foo/bar/baz.rs")),
            "bar/baz.rs"
        );
        assert_eq!(
            relative_path(Path::new("/foo/"), Path::new("/foo/bar/baz/")),
            "bar/baz"
        );
    }

    #[test]
    fn test_relative_path_not_ancestor() {
        // Base is not ancestor - return full target path
        assert_eq!(
            relative_path(
                Path::new("/foo/bar"),
                Path::new("/completely/different/file.rs")
            ),
            "/completely/different/file.rs"
        );
        assert_eq!(
            relative_path(Path::new("/foo/bar/"), Path::new("/foo/other.rs")),
            "/foo/other.rs"
        );
    }

    #[test]
    fn test_relative_path_parent_to_child() {
        // Going from parent to deeper descendant
        assert_eq!(
            relative_path(Path::new("/foo"), Path::new("/foo/bar/baz/qux.rs")),
            "bar/baz/qux.rs"
        );
    }

    #[test]
    fn test_relative_path_root_cases() {
        // From root
        assert_eq!(
            relative_path(Path::new("/"), Path::new("/foo/bar.rs")),
            "foo/bar.rs"
        );
        // Root to root
        assert_eq!(relative_path(Path::new("/"), Path::new("/")), ".");
    }

    #[test]
    fn test_relative_path_no_trailing_slash() {
        // Test ambiguous cases without trailing slash
        assert_eq!(
            relative_path(Path::new("/foo/bar"), Path::new("/foo/bar/baz")),
            "baz"
        );
    }

    #[test]
    fn test_relative_path_relative_paths() {
        // Both paths are relative (no leading slash)
        assert_eq!(
            relative_path(Path::new("foo/bar"), Path::new("foo/bar/baz.rs")),
            "baz.rs"
        );
        assert_eq!(
            relative_path(Path::new("foo"), Path::new("bar/baz.rs")),
            "bar/baz.rs" // Not an ancestor, return full path
        );
    }

    #[test]
    fn test_vec() {
        let numbers = vec![1, 2, 2, 3, 4, 4, 4, 5];
        let grouped = group_by(numbers, |a, b| a == b);
        assert_eq!(
            grouped,
            vec![vec![1], vec![2, 2], vec![3], vec![4, 4, 4], vec![5]]
        );
    }

    #[test]
    fn test_array() {
        let numbers = [1, 2, 2, 3, 4, 4, 4, 5];
        let grouped = group_by(numbers, |a, b| a == b);
        assert_eq!(
            grouped,
            vec![vec![1], vec![2, 2], vec![3], vec![4, 4, 4], vec![5]]
        );
    }

    #[test]
    fn test_range() {
        let grouped = group_by(1..=5, |a, b| b - a <= 1);
        assert_eq!(grouped, vec![vec![1, 2, 3, 4, 5]]);
    }

    #[test]
    fn test_chain() {
        let first = [1, 2];
        let second = [2, 3];
        let grouped = group_by(first.into_iter().chain(second), |a, b| a == b);
        assert_eq!(grouped, vec![vec![1], vec![2, 2], vec![3]]);
    }

    #[test]
    fn test_empty() {
        let empty: Vec<i32> = vec![];
        let grouped = group_by(empty, |a, b| a == b);
        assert_eq!(grouped, Vec::<Vec<i32>>::new());
    }

    #[test]
    fn test_single() {
        let single = std::iter::once(1);
        let grouped = group_by(single, |a, b| a == b);
        assert_eq!(grouped, vec![vec![1]]);
    }

    #[test]
    fn test_string_slice() {
        let words = ["apple", "app", "banana", "ban", "cat"];
        let grouped = group_by(words, |a, b| a.starts_with(b) || b.starts_with(a));
        assert_eq!(
            grouped,
            vec![vec!["apple", "app"], vec!["banana", "ban"], vec!["cat"]]
        );
    }

    #[test]
    fn test_read_lines_in_range() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 1, 3).unwrap().collect::<Vec<_>>();

        assert_eq!(
            result,
            vec![
                (1, "line2".to_string()),
                (2, "line3".to_string()),
                (3, "line4".to_string())
            ]
        );
    }

    #[test]
    fn test_read_single_line() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 2, 2).unwrap().collect::<Vec<_>>();

        assert_eq!(result, vec![(2, "line3".to_string())]);
    }

    #[test]
    fn test_read_from_beginning() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 0, 2).unwrap().collect::<Vec<_>>();

        assert_eq!(
            result,
            vec![
                (0, "line1".to_string()),
                (1, "line2".to_string()),
                (2, "line3".to_string())
            ]
        );
    }

    #[test]
    fn test_read_to_end() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 3, 4).unwrap().collect::<Vec<_>>();

        assert_eq!(
            result,
            vec![(3, "line4".to_string()), (4, "line5".to_string())]
        );
    }

    #[test]
    fn test_read_all_lines() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 0, 4).unwrap().collect::<Vec<_>>();

        assert_eq!(
            result,
            vec![
                (0, "line1".to_string()),
                (1, "line2".to_string()),
                (2, "line3".to_string()),
                (3, "line4".to_string()),
                (4, "line5".to_string())
            ]
        );
    }

    #[test]
    fn test_empty_file() {
        let contents = "";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 0, 2).unwrap().collect::<Vec<_>>();

        assert_eq!(result, Vec::<(usize, String)>::new());
    }

    #[test]
    fn test_range_exceeds_file_length() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 1, 10).unwrap().collect::<Vec<_>>();

        assert_eq!(
            result,
            vec![(1, "line2".to_string()), (2, "line3".to_string())]
        );
    }

    #[test]
    fn test_start_exceeds_file_length() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 5, 10).unwrap().collect::<Vec<_>>();

        assert_eq!(result, Vec::<(usize, String)>::new());
    }

    #[test]
    #[should_panic(expected = "Expected start <= end")]
    fn test_start_greater_than_end() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);
        let path = file.path();

        let _ = read_lines_range(path, 3, 1);
    }

    #[test]
    fn test_with_empty_lines() {
        let contents = "line1\n\nline3\n\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 0, 4).unwrap().collect::<Vec<_>>();

        assert_eq!(
            result,
            vec![
                (0, "line1".to_string()),
                (1, "".to_string()),
                (2, "line3".to_string()),
                (3, "".to_string()),
                (4, "line5".to_string())
            ]
        );
    }

    #[test]
    fn test_file_not_found() {
        let path = Path::new("non_existent_file.txt");

        let result = read_lines_range(path, 0, 5);

        assert!(result.is_err());
    }

    #[test]
    fn test_largest_window_around() {
        assert!(largest_range_centered_on(5, 0, 0, NonZeroUsize::new(1).unwrap()).is_err());

        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(1).unwrap()).unwrap(),
            (5, 5)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(2).unwrap()).unwrap(),
            (5, 6)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(3).unwrap()).unwrap(),
            (4, 6)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(4).unwrap()).unwrap(),
            (4, 7)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(5).unwrap()).unwrap(),
            (3, 7)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(6).unwrap()).unwrap(),
            (3, 8)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(7).unwrap()).unwrap(),
            (2, 8)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(8).unwrap()).unwrap(),
            (2, 9)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(9).unwrap()).unwrap(),
            (1, 9)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(10).unwrap()).unwrap(),
            (0, 9)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(11).unwrap()).unwrap(),
            (0, 9)
        );
        assert_eq!(
            largest_range_centered_on(5, 0, 9, NonZeroUsize::new(999).unwrap()).unwrap(),
            (0, 9)
        );

        assert_eq!(
            largest_range_centered_on(0, 0, 9, NonZeroUsize::new(1).unwrap()).unwrap(),
            (0, 0)
        );
        assert_eq!(
            largest_range_centered_on(0, 0, 9, NonZeroUsize::new(2).unwrap()).unwrap(),
            (0, 1)
        );
        assert_eq!(
            largest_range_centered_on(0, 0, 9, NonZeroUsize::new(100).unwrap()).unwrap(),
            (0, 9)
        );

        assert_eq!(
            largest_range_centered_on(5, 3, 5, NonZeroUsize::new(1).unwrap()).unwrap(),
            (5, 5)
        );
        assert_eq!(
            largest_range_centered_on(5, 3, 5, NonZeroUsize::new(2).unwrap()).unwrap(),
            (4, 5)
        );
        assert_eq!(
            largest_range_centered_on(5, 3, 5, NonZeroUsize::new(100).unwrap()).unwrap(),
            (3, 5)
        );
    }

    #[test]
    fn test_sanitize_normal_text() {
        assert_eq!(strip_control_chars("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_tabs() {
        assert_eq!(strip_control_chars("hello\tworld"), "hello  world");
        assert_eq!(strip_control_chars("\t\t"), "    ");
    }

    #[test]
    fn test_sanitize_newlines() {
        assert_eq!(strip_control_chars("hello\nworld"), "hello world");
        assert_eq!(strip_control_chars("\n\n"), "  ");
    }

    #[test]
    fn test_sanitize_control_chars() {
        assert_eq!(strip_control_chars("hello\u{4}world"), "hello�world");
        assert_eq!(strip_control_chars("test\u{7}"), "test�");
        assert_eq!(strip_control_chars("\u{1b}[0m"), "�[0m");
    }

    #[test]
    fn test_sanitize_unicode() {
        assert_eq!(strip_control_chars("héllo→世界"), "héllo→世界");
    }

    #[test]
    fn test_sanitize_empty_string() {
        assert_eq!(strip_control_chars(""), "");
    }

    #[test]
    fn test_sanitize_only_control_chars() {
        assert_eq!(strip_control_chars("\u{1}\u{2}\u{3}\u{4}"), "����");
    }

    #[test]
    fn test_ceil_div() {
        assert_eq!(ceil_div(1, 1), 1);
        assert_eq!(ceil_div(2, 1), 2);
        assert_eq!(ceil_div(1, 2), 1);
        assert_eq!(ceil_div(0, 1), 0);
        assert_eq!(ceil_div(2, 3), 1);
        assert_eq!(ceil_div(27, 4), 7);
        assert_eq!(ceil_div(26, 9), 3);
        assert_eq!(ceil_div(27, 9), 3);
        assert_eq!(ceil_div(28, 9), 4);
        assert_eq!(ceil_div(-2, 3), 0);
        assert_eq!(ceil_div(-4, 3), -1);
        assert_eq!(ceil_div(2, -3), 0);
        assert_eq!(ceil_div(4, -3), -1);
        assert_eq!(ceil_div(-2, -3), 1);
        assert_eq!(ceil_div(-4, -3), 2);
    }

    fn get_theme() -> Theme {
        let ts = ThemeSet::load_defaults();
        ts.themes["base16-ocean.dark"].clone()
    }

    fn get_syntax() -> SyntaxSet {
        two_face::syntax::extra_no_newlines()
    }

    #[allow(clippy::type_complexity)]
    fn extract_text(
        highlighted_lines: &[(usize, Vec<(Option<Style>, String)>)],
    ) -> Vec<(usize, String)> {
        highlighted_lines
            .iter()
            .map(|(idx, styles)| {
                let text = styles
                    .iter()
                    .map(|(_, text)| text.clone())
                    .collect::<String>();
                (*idx, text)
            })
            .collect()
    }

    #[test]
    fn test_read_lines_in_range_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3\nline4\nline5";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(1),
                Some(3),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(
                extract_text(&result),
                vec![
                    (1, "line2".to_string()),
                    (2, "line3".to_string()),
                    (3, "line4".to_string())
                ]
            );
        }
    }

    #[test]
    fn test_read_single_line_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3\nline4\nline5";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(2),
                Some(2),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(extract_text(&result), vec![(2, "line3".to_string())]);
        }
    }

    #[test]
    fn test_read_from_beginning_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3\nline4\nline5";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(0),
                Some(2),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(
                extract_text(&result),
                vec![
                    (0, "line1".to_string()),
                    (1, "line2".to_string()),
                    (2, "line3".to_string())
                ]
            );
        }
    }

    #[test]
    fn test_read_to_end_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3\nline4\nline5";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(3),
                Some(4),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(
                extract_text(&result),
                vec![(3, "line4".to_string()), (4, "line5".to_string())]
            );
        }
    }

    #[test]
    fn test_read_all_lines_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3\nline4\nline5";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(0),
                Some(4),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(
                extract_text(&result),
                vec![
                    (0, "line1".to_string()),
                    (1, "line2".to_string()),
                    (2, "line3".to_string()),
                    (3, "line4".to_string()),
                    (4, "line5".to_string())
                ]
            );
        }
    }

    #[test]
    fn test_empty_file_highlighted() {
        for full in [true, false] {
            let contents = "";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(0),
                Some(2),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(extract_text(&result), Vec::<(usize, String)>::new());
        }
    }

    #[test]
    fn test_range_exceeds_file_length_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(1),
                Some(10),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(
                extract_text(&result),
                vec![(1, "line2".to_string()), (2, "line3".to_string())]
            );
        }
    }

    #[test]
    fn test_start_exceeds_file_length_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(5),
                Some(10),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(extract_text(&result), Vec::<(usize, String)>::new());
        }
    }

    #[test]
    #[should_panic(expected = "Expected start <= end")]
    fn test_start_greater_than_end_highlighted() {
        for full in [true, false] {
            let contents = "line1\nline2\nline3";
            let file = create_test_file(contents);
            let _ = read_lines_range_highlighted(
                file.path(),
                Some(3),
                Some(1),
                &get_theme(),
                &get_syntax(),
                full,
            );
        }
    }

    #[test]
    fn test_with_empty_lines_highlighted() {
        for full in [true, false] {
            let contents = "line1\n\nline3\n\nline5";
            let file = create_test_file(contents);

            let result = read_lines_range_highlighted(
                file.path(),
                Some(0),
                Some(4),
                &get_theme(),
                &get_syntax(),
                full,
            )
            .unwrap()
            .collect::<Vec<_>>();

            assert_eq!(
                extract_text(&result),
                vec![
                    (0, "line1".to_string()),
                    (1, "".to_string()),
                    (2, "line3".to_string()),
                    (3, "".to_string()),
                    (4, "line5".to_string())
                ]
            );
        }
    }

    #[test]
    fn test_file_not_found_highlighted() {
        let path = Path::new("non_existent_file.txt");

        let theme = get_theme();
        let syntax = get_syntax();
        let result = read_lines_range_highlighted(path, Some(0), Some(5), &theme, &syntax, true);

        assert!(result.is_err());
    }

    #[test]
    fn test_highlighting_preserves_content() {
        let contents = "fn main() {\n    println!(\"Hello, world!\");\n}";
        let file = create_test_file(contents);

        let result = read_lines_range_highlighted(
            file.path(),
            Some(0),
            Some(2),
            &get_theme(),
            &get_syntax(),
            true,
        )
        .unwrap()
        .collect::<Vec<_>>();

        assert_eq!(
            extract_text(&result),
            vec![
                (0, "fn main() {".to_string()),
                (1, "    println!(\"Hello, world!\");".to_string()),
                (2, "}".to_string())
            ]
        );
    }

    #[test]
    fn test_split_empty() {
        let empty: Vec<i32> = vec![];
        let (prefix, rest) = split_while(&empty, |&x| x > 0);
        assert_eq!(prefix, &[] as &[i32]);
        assert_eq!(rest, &[] as &[i32]);
    }

    #[test]
    fn test_all_satisfy() {
        let vec = vec![1, 2, 3, 4, 5];
        let (prefix, rest) = split_while(&vec, |&x| x > 0);
        assert_eq!(prefix, &[1, 2, 3, 4, 5]);
        assert_eq!(rest, &[] as &[i32]);
    }

    #[test]
    fn test_none_satisfy() {
        let vec = vec![1, 2, 3, 4, 5];
        let (prefix, rest) = split_while(&vec, |&x| x > 5);
        assert_eq!(prefix, &[] as &[i32]);
        assert_eq!(rest, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_some_satisfy() {
        let vec = vec![1, 2, 3, 4, 5];
        let (prefix, rest) = split_while(&vec, |&x| x < 3);
        assert_eq!(prefix, &[1, 2]);
        assert_eq!(rest, &[3, 4, 5]);
    }

    #[test]
    fn test_only_first_satisfies() {
        let vec = vec![1, 2, 3, 4, 5];
        let (prefix, rest) = split_while(&vec, |&x| x == 1);
        assert_eq!(prefix, &[1]);
        assert_eq!(rest, &[2, 3, 4, 5]);
    }

    #[test]
    fn test_only_last_fails() {
        let vec = vec![1, 2, 3, 4, 5];
        let (prefix, rest) = split_while(&vec, |&x| x < 5);
        assert_eq!(prefix, &[1, 2, 3, 4]);
        assert_eq!(rest, &[5]);
    }

    #[test]
    fn test_with_strings() {
        let vec = vec!["apple", "banana", "cherry", "date", "elderberry"];
        let (prefix, rest) = split_while(&vec, |&s| s.starts_with('a') || s.starts_with('b'));
        assert_eq!(prefix, &["apple", "banana"]);
        assert_eq!(rest, &["cherry", "date", "elderberry"]);
    }

    #[test]
    fn test_owned_vec() {
        // Test for the owned version if you implemented it
        let vec = vec![1, 2, 3, 4, 5];
        let (prefix, rest) = split_while(&vec, |&x| x < 3);
        assert_eq!(prefix, vec![1, 2]);
        assert_eq!(rest, vec![3, 4, 5]);
    }

    #[test]
    fn test_last_n_empty() {
        let vec: Vec<usize> = vec![];
        assert_eq!(last_n(&vec, 0), &(vec![] as Vec<usize>));
        assert_eq!(last_n(&vec, 3), &(vec![] as Vec<usize>));
    }

    #[test]
    fn test_last_n_non_empty() {
        let vec = (0..10).collect::<Vec<usize>>();
        assert_eq!(last_n(&vec, 0), &(vec![] as Vec<usize>));
        assert_eq!(last_n(&vec, 1), &vec![9]);
        assert_eq!(last_n(&vec, 3), &vec![7, 8, 9]);
        assert_eq!(last_n(&vec, 10), &vec);
        assert_eq!(last_n(&vec, 200), &vec);
    }

    #[test]
    fn test_last_string_n_empty() {
        let s = "".chars().collect::<Vec<_>>();
        assert_eq!(last_n(&s, 0), &(vec![] as Vec<char>));
        assert_eq!(last_n(&s, 3), &(vec![] as Vec<char>));
    }

    #[test]
    fn test_last_n_string_non_empty() {
        let s = "abcdefghijkl".chars().collect::<Vec<_>>();
        assert_eq!(last_n(&s, 0), "".chars().collect::<Vec<_>>());
        assert_eq!(last_n(&s, 1), "l".chars().collect::<Vec<_>>());
        assert_eq!(last_n(&s, 3), "jkl".chars().collect::<Vec<_>>());
        assert_eq!(last_n(&s, 10), "cdefghijkl".chars().collect::<Vec<_>>());
        assert_eq!(last_n(&s, 12), "abcdefghijkl".chars().collect::<Vec<_>>());
        assert_eq!(last_n(&s, 13), "abcdefghijkl".chars().collect::<Vec<_>>());
        assert_eq!(last_n(&s, 200), "abcdefghijkl".chars().collect::<Vec<_>>());
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(last_n_chars("", 5), "");
    }

    #[test]
    fn test_zero_chars() {
        assert_eq!(last_n_chars("hello", 0), "");
    }

    #[test]
    fn test_ascii_full_string() {
        assert_eq!(last_n_chars("hello", 5), "hello");
    }

    #[test]
    fn test_ascii_partial_string() {
        assert_eq!(last_n_chars("hello", 3), "llo");
    }

    #[test]
    fn test_ascii_more_than_string() {
        assert_eq!(last_n_chars("hello", 10), "hello");
    }

    #[test]
    fn test_unicode_chars() {
        assert_eq!(last_n_chars("héllö wörld", 5), "wörld");
    }

    #[test]
    fn test_multibyte_chars() {
        let s = "こんにちは世界";
        assert_eq!(last_n_chars(s, 2), "世界");
    }

    #[test]
    fn test_emoji() {
        let s = "Hello 👋 World 🌍";
        assert_eq!(last_n_chars(s, 9), "👋 World 🌍");
    }

    #[allow(clippy::similar_names)]
    #[test]
    fn test_typescript_syntax_available() {
        let syntax_set = two_face::syntax::extra_no_newlines();

        let ts_syntax = syntax_set.find_syntax_by_extension("ts");
        assert!(ts_syntax.is_some(), "TypeScript syntax should be available");
        assert_eq!(ts_syntax.unwrap().name, "TypeScript");

        let tsx_syntax = syntax_set.find_syntax_by_extension("tsx");
        assert!(
            tsx_syntax.is_some(),
            "TypeScript React syntax should be available"
        );
        assert_eq!(tsx_syntax.unwrap().name, "TypeScriptReact");

        let js_syntax = syntax_set.find_syntax_by_extension("js");
        assert!(js_syntax.is_some(), "JavaScript syntax should be available");
    }
}
