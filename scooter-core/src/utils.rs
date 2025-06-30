use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
