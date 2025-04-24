use anyhow::{anyhow, Result};
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    ops::{Add, Div, Mul, Rem},
    path::{Path, PathBuf},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, Theme},
    parsing::SyntaxSet,
};

pub fn replace_start(s: String, from: &str, to: &str) -> String {
    if let Some(stripped) = s.strip_prefix(from) {
        format!("{}{}", to, stripped)
    } else {
        s.to_string()
    }
}

pub fn relative_path_from(root_dir: &Path, path: &Path) -> String {
    let root_dir = root_dir.to_str().unwrap();
    let path = path.to_str().unwrap().to_owned();
    replace_start(path, root_dir, ".")
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

pub fn validate_directory(dir_str: &str) -> Result<PathBuf> {
    let path = Path::new(dir_str);
    if path.exists() {
        Ok(path.to_path_buf())
    } else {
        Err(anyhow!(
            "Directory '{}' does not exist. Please provide a valid directory path.",
            dir_str
        ))
    }
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

pub fn read_lines_range(path: &Path, start: usize, end: usize) -> io::Result<Vec<(usize, String)>> {
    if start > end {
        panic!("Expected start <= end, found start={start}, end={end}");
    }
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let lines = reader
        .lines()
        .enumerate()
        .skip(start)
        .take(end - start + 1)
        .map(|(idx, line)| (idx, line.unwrap()))
        .collect();
    Ok(lines)
}

static SYNTAX_HIGHLIGHTING_CONTEXT_LEN: usize = 20;

#[allow(clippy::type_complexity)]
pub fn read_lines_range_highlighted(
    path: &Path,
    start: usize,
    end: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
) -> io::Result<Vec<(usize, Vec<(Option<Style>, String)>)>> {
    if start > end {
        panic!("Expected start <= end, found start={start}, end={end}");
    }

    let syntax_ref = syntax_set
        .find_syntax_for_file(path)?
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax_ref, theme);

    // Get additional lines before start for context when syntax highlighting - ideally we would read
    // entire file but this would be slow
    let lines = read_lines_range(
        path,
        start.saturating_sub(SYNTAX_HIGHLIGHTING_CONTEXT_LEN),
        end,
    )?;
    let lines = lines
        .iter()
        .skip_while(|(idx, _)| *idx < start)
        .take(end - start + 1)
        .map(|(idx, line)| {
            let highlighted = match highlighter.highlight_line(line, syntax_set) {
                Ok(l) => l
                    .into_iter()
                    .map(|(style, text)| (Some(style), text.to_string()))
                    .collect(),
                Err(e) => {
                    log::error!("Error when highlighting line {line}: {e}");
                    vec![(None, line.clone())]
                }
            };
            (*idx, highlighted)
        })
        .collect();

    Ok(lines)
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

/// Returns the largest range centred on `centre` that is both within `min_bound` and `max_bound`,
/// and no larger than `max_size`.
///
/// # Example
/// ```
/// use scooter::utils::largest_range_centered_on;
///
/// // Simple case - centered range with room on both sides
/// let (start, end) = largest_range_centered_on(5, 0, 10, 5);
/// assert_eq!((3, 7), (start, end));
///
/// // Range limited by lower bound
/// let (start, end) = largest_range_centered_on(0, 0, 10, 5);
/// assert_eq!((0, 4), (start, end));
///
/// // Range limited by upper bound
/// let (start, end) = largest_range_centered_on(8, 0, 10, 5);
/// assert_eq!((6, 10), (start, end));
///
/// // Range limited by max_size
/// let (start, end) = largest_range_centered_on(5, 0, 20, 3);
/// assert_eq!((4, 6), (start, end));
/// ```
pub fn largest_range_centered_on(
    centre: usize,
    lower_bound: usize,
    upper_bound: usize,
    max_size: usize,
) -> (usize, usize) {
    if centre < lower_bound || centre > upper_bound {
        panic!(
            "Expected start<=pos<=end, found start={lower_bound}, pos={centre}, end={upper_bound}"
        );
    }
    if max_size == 0 {
        panic!("Expected max_size > 0, found {max_size}");
    }

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

    (cur_start, cur_end)
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
    use std::{fs, io::Write};
    use syntect::highlighting::ThemeSet;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_replace_start_matching_prefix() {
        assert_eq!(replace_start("abac".to_string(), "a", "z"), "zbac");
    }

    #[test]
    fn test_replace_start_no_match() {
        assert_eq!(replace_start("bac".to_string(), "a", "z"), "bac");
    }

    #[test]
    fn test_replace_start_empty_string() {
        assert_eq!(replace_start("".to_string(), "a", "z"), "");
    }

    #[test]
    fn test_replace_start_longer_prefix() {
        assert_eq!(
            replace_start("hello world hello there".to_string(), "hello", "hi"),
            "hi world hello there"
        );
    }

    #[test]
    fn test_replace_start_whole_string() {
        assert_eq!(replace_start("abc".to_string(), "abc", "xyz"), "xyz");
    }

    #[test]
    fn test_replace_start_empty_from() {
        assert_eq!(replace_start("abc".to_string(), "", "xyz"), "xyzabc");
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

    fn setup_test_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_validate_directory_exists() {
        let temp_dir = setup_test_dir();
        let dir_path = temp_dir.path().to_str().unwrap();

        let result = validate_directory(dir_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from(dir_path));
    }

    #[test]
    fn test_validate_directory_does_not_exist() {
        let nonexistent_path = "/path/that/definitely/does/not/exist/12345";
        let result = validate_directory(nonexistent_path);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"));
        assert!(err.contains(nonexistent_path));
    }

    #[test]
    fn test_validate_directory_with_nested_structure() {
        let temp_dir = setup_test_dir();
        let nested_dir = temp_dir.path().join("nested").join("directory");
        fs::create_dir_all(&nested_dir).expect("Failed to create nested directories");

        let dir_path = nested_dir.to_str().unwrap();
        let result = validate_directory(dir_path);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), nested_dir);
    }

    #[test]
    fn test_validate_directory_with_special_chars() {
        let temp_dir = setup_test_dir();
        let special_dir = temp_dir.path().join("test with spaces and-symbols_!@#$");
        fs::create_dir(&special_dir).expect("Failed to create directory with special characters");

        let dir_path = special_dir.to_str().unwrap();
        let result = validate_directory(dir_path);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), special_dir);
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

    fn create_test_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    #[test]
    fn test_read_lines_in_range() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 1, 3).unwrap();

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

        let result = read_lines_range(path, 2, 2).unwrap();

        assert_eq!(result, vec![(2, "line3".to_string())]);
    }

    #[test]
    fn test_read_from_beginning() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 0, 2).unwrap();

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

        let result = read_lines_range(path, 3, 4).unwrap();

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

        let result = read_lines_range(path, 0, 4).unwrap();

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

        let result = read_lines_range(path, 0, 2).unwrap();

        assert_eq!(result, Vec::<(usize, String)>::new());
    }

    #[test]
    fn test_range_exceeds_file_length() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);
        let path = file.path();

        let result = read_lines_range(path, 1, 10).unwrap();

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

        let result = read_lines_range(path, 5, 10).unwrap();

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

        let result = read_lines_range(path, 0, 4).unwrap();

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
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 1, 3, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(
            extract_text(&result),
            vec![
                (1, "line2".to_string()),
                (2, "line3".to_string()),
                (3, "line4".to_string())
            ]
        );
    }

    #[test]
    fn test_read_single_line_highlighted() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 2, 2, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(extract_text(&result), vec![(2, "line3".to_string())]);
    }

    #[test]
    fn test_read_from_beginning_highlighted() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 0, 2, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(
            extract_text(&result),
            vec![
                (0, "line1".to_string()),
                (1, "line2".to_string()),
                (2, "line3".to_string())
            ]
        );
    }

    #[test]
    fn test_read_to_end_highlighted() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 3, 4, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(
            extract_text(&result),
            vec![(3, "line4".to_string()), (4, "line5".to_string())]
        );
    }

    #[test]
    fn test_read_all_lines_highlighted() {
        let contents = "line1\nline2\nline3\nline4\nline5";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 0, 4, &get_theme(), &get_syntax()).unwrap();

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

    #[test]
    fn test_empty_file_highlighted() {
        let contents = "";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 0, 2, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(extract_text(&result), Vec::<(usize, String)>::new());
    }

    #[test]
    fn test_range_exceeds_file_length_highlighted() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 1, 10, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(
            extract_text(&result),
            vec![(1, "line2".to_string()), (2, "line3".to_string())]
        );
    }

    #[test]
    fn test_start_exceeds_file_length_highlighted() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 5, 10, &get_theme(), &get_syntax()).unwrap();

        assert_eq!(extract_text(&result), Vec::<(usize, String)>::new());
    }

    #[test]
    #[should_panic(expected = "Expected start <= end")]
    fn test_start_greater_than_end_highlighted() {
        let contents = "line1\nline2\nline3";
        let file = create_test_file(contents);
        let _ = read_lines_range_highlighted(file.path(), 3, 1, &get_theme(), &get_syntax());
    }

    #[test]
    fn test_with_empty_lines_highlighted() {
        let contents = "line1\n\nline3\n\nline5";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 0, 4, &get_theme(), &get_syntax()).unwrap();

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

    #[test]
    fn test_file_not_found_highlighted() {
        let path = Path::new("non_existent_file.txt");

        let result = read_lines_range_highlighted(path, 0, 5, &get_theme(), &get_syntax());

        assert!(result.is_err());
    }

    #[test]
    fn test_highlighting_preserves_content() {
        let contents = "fn main() {\n    println!(\"Hello, world!\");\n}";
        let file = create_test_file(contents);

        let result =
            read_lines_range_highlighted(file.path(), 0, 2, &get_theme(), &get_syntax()).unwrap();

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
    fn test_largest_window_around() {
        assert_eq!(largest_range_centered_on(5, 0, 9, 1), (5, 5));
        assert_eq!(largest_range_centered_on(5, 0, 9, 2), (5, 6));
        assert_eq!(largest_range_centered_on(5, 0, 9, 3), (4, 6));
        assert_eq!(largest_range_centered_on(5, 0, 9, 4), (4, 7));
        assert_eq!(largest_range_centered_on(5, 0, 9, 5), (3, 7));
        assert_eq!(largest_range_centered_on(5, 0, 9, 6), (3, 8));
        assert_eq!(largest_range_centered_on(5, 0, 9, 7), (2, 8));
        assert_eq!(largest_range_centered_on(5, 0, 9, 8), (2, 9));
        assert_eq!(largest_range_centered_on(5, 0, 9, 9), (1, 9));
        assert_eq!(largest_range_centered_on(5, 0, 9, 10), (0, 9));
        assert_eq!(largest_range_centered_on(5, 0, 9, 11), (0, 9));
        assert_eq!(largest_range_centered_on(5, 0, 9, 999), (0, 9));

        assert_eq!(largest_range_centered_on(0, 0, 9, 1), (0, 0));
        assert_eq!(largest_range_centered_on(0, 0, 9, 2), (0, 1));
        assert_eq!(largest_range_centered_on(0, 0, 9, 100), (0, 9));

        assert_eq!(largest_range_centered_on(5, 3, 5, 1), (5, 5));
        assert_eq!(largest_range_centered_on(5, 3, 5, 2), (4, 5));
        assert_eq!(largest_range_centered_on(5, 3, 5, 100), (3, 5));
    }
}
