use std::{
    fs::File,
    io::{self, BufReader},
    ops::{Add, Div, Mul, Rem},
    path::Path,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, Theme},
    parsing::SyntaxSet,
};

use frep_core::line_reader::{BufReadExt, LinesSplitEndings};

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
        if let Some(end) = self.end_idx {
            if self.current_idx > end {
                return None;
            }
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

#[allow(dead_code)]
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

#[macro_export]
macro_rules! test_with_both_regex_modes {
    ($name:ident, $test_fn:expr) => {
        mod $name {
            use super::*;
            use serial_test::serial;

            #[tokio::test]
            #[serial]
            async fn with_advanced_regex() -> anyhow::Result<()> {
                ($test_fn)(true).await
            }

            #[tokio::test]
            #[serial]
            async fn without_advanced_regex() -> anyhow::Result<()> {
                ($test_fn)(false).await
            }
        }
    };
}

#[macro_export]
macro_rules! test_with_both_regex_modes_and_fixed_strings {
    ($name:ident, $test_fn:expr) => {
        mod $name {
            use super::*;
            use serial_test::serial;

            #[tokio::test]
            #[serial]
            async fn with_advanced_regex_no_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(true, false).await
            }

            #[tokio::test]
            #[serial]
            async fn with_advanced_regex_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(true, true).await
            }

            #[tokio::test]
            #[serial]
            async fn without_advanced_regex_no_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(false, false).await
            }

            #[tokio::test]
            #[serial]
            async fn without_advanced_regex_fixed_strings() -> anyhow::Result<()> {
                ($test_fn)(false, true).await
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use syntect::highlighting::ThemeSet;
    use tempfile::NamedTempFile;

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

    fn create_test_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    fn get_theme() -> Theme {
        let ts = ThemeSet::load_defaults();
        ts.themes["base16-ocean.dark"].clone()
    }

    fn get_syntax() -> SyntaxSet {
        SyntaxSet::load_defaults_nonewlines()
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
}
