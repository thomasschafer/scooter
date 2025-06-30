use std::{
    fs::File,
    io::{self, BufReader},
    num::NonZeroUsize,
    path::Path,
};

use anyhow::{bail, Context};
use frep_core::line_reader::BufReadExt;

pub fn replace_start(s: &str, from: &str, to: &str) -> String {
    if let Some(stripped) = s.strip_prefix(from) {
        format!("{to}{stripped}")
    } else {
        s.to_string()
    }
}

pub fn relative_path_from(root_dir: &Path, path: &Path) -> String {
    let root_dir = root_dir.to_string_lossy();
    let path = path.to_string_lossy();
    replace_start(&path, &root_dir, ".")
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

pub fn read_lines_range(
    path: &Path,
    start: usize,
    end: usize,
) -> io::Result<impl Iterator<Item = (usize, String)>> {
    assert!(
        start <= end,
        "Expected start <= end, found start={start}, end={end}"
    );

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let lines = reader
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
        });

    Ok(lines)
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
    num_lines_to_show: usize,
) -> anyhow::Result<(Vec<(usize, T)>, (usize, T), Vec<(usize, T)>)> {
    let file_start = indexed_lines.first().context("No lines found")?.0;
    let file_end = indexed_lines.last().context("No lines found")?.0;
    let (new_start, new_end) = largest_range_centered_on(
        line_idx,
        file_start,
        file_end,
        NonZeroUsize::new(num_lines_to_show).context("preview will have height 0")?,
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use tempfile::NamedTempFile;

    use super::*;

    fn create_test_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    #[test]
    fn test_replace_start_matching_prefix() {
        assert_eq!(replace_start("abac", "a", "z"), "zbac");
    }

    #[test]
    fn test_replace_start_no_match() {
        assert_eq!(replace_start("bac", "a", "z"), "bac");
    }

    #[test]
    fn test_replace_start_empty_string() {
        assert_eq!(replace_start("", "a", "z"), "");
    }

    #[test]
    fn test_replace_start_longer_prefix() {
        assert_eq!(
            replace_start("hello world hello there", "hello", "hi"),
            "hi world hello there"
        );
    }

    #[test]
    fn test_replace_start_whole_string() {
        assert_eq!(replace_start("abc", "abc", "xyz"), "xyz");
    }

    #[test]
    fn test_replace_start_empty_from() {
        assert_eq!(replace_start("abc", "", "xyz"), "xyzabc");
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
}
