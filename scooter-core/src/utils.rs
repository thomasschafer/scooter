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
}
