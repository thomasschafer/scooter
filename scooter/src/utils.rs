use anyhow::{anyhow, Result};
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    ops::{Add, Div, Rem},
    path::{Path, PathBuf},
};
use syntect::{
    easy::HighlightFile,
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
        + Rem<Output = T>
        + From<bool>
        + From<u8>
        + PartialEq
        + Copy,
{
    a / b + T::from((a % b) != T::from(0))
}

pub fn read_lines_range(path: &Path, start: usize, end: usize) -> io::Result<Vec<String>> {
    if start > end {
        panic!("Expected start <= end, found start={start}, end={end}");
    }
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let lines = reader
        .lines()
        .skip(start)
        .take(end - start + 1)
        .map(|line| line.unwrap())
        .collect();
    Ok(lines)
}

pub fn read_lines_range_highlighted(
    path: &Path,
    start: usize,
    end: usize,
    theme: &Theme,
) -> io::Result<Vec<Vec<(Style, String)>>> {
    if start > end {
        panic!("Expected start <= end, found start={start}, end={end}");
    }

    let ss = SyntaxSet::load_defaults_nonewlines();
    let mut highlighter = HighlightFile::new(path, &ss, theme)?;

    let lines = highlighter
        .reader
        .lines()
        .skip(start)
        .take(end - start + 1)
        .map(|line| {
            let line = line.unwrap();
            highlighter
                .highlight_lines
                .highlight_line(&line, &ss)
                .unwrap()
                .into_iter()
                .map(|(style, text)| (style, text.to_string()))
                .collect()
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
    use std::fs;
    use tempfile::TempDir;

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
        assert_eq!(ceil_div(-2, 3), 0);
        assert_eq!(ceil_div(27, 4), 7);
        assert_eq!(ceil_div(26, 9), 3);
        assert_eq!(ceil_div(27, 9), 3);
        assert_eq!(ceil_div(28, 9), 4);
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
}
