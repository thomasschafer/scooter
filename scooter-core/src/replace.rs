use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, Write},
};
use tempfile::NamedTempFile;

use crate::line_reader::BufReadExt;
use crate::search::SearchResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceResult {
    Success,
    Error(String),
}

// TODO: document - e.g. should be called on results from same file.
pub fn replace_in_file(results: &mut [SearchResult]) -> anyhow::Result<()> {
    let file_path = match results {
        [r, ..] => r.path.clone(),
        [] => return Ok(()),
    };
    debug_assert!(results.iter().all(|r| r.path == file_path));

    let mut line_map: HashMap<_, _> = results
        .iter_mut()
        .map(|res| (res.line_number, res))
        .collect();

    let parent_dir = file_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot create temp file: target path '{}' has no parent directory",
            file_path.display()
        )
    })?;
    let temp_output_file = NamedTempFile::new_in(parent_dir)?;

    // Scope the file operations so they're closed before rename
    {
        let input = File::open(file_path.clone())?;
        let reader = BufReader::new(input);

        let output = File::create(temp_output_file.path())?;
        let mut writer = BufWriter::new(output);

        for (mut line_number, line_result) in reader.lines_with_endings().enumerate() {
            line_number += 1; // Ensure line-number is 1-indexed
            let (mut line, line_ending) = line_result?;
            if let Some(res) = line_map.get_mut(&line_number) {
                if line == res.line.as_bytes() {
                    line = res.replacement.as_bytes().to_vec();
                    res.replace_result = Some(ReplaceResult::Success);
                } else {
                    res.replace_result = Some(ReplaceResult::Error(
                        "File changed since last search".to_owned(),
                    ));
                }
            }
            line.extend(line_ending.as_bytes());
            writer.write_all(&line)?;
        }

        writer.flush()?;
    }

    temp_output_file.persist(file_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_reader::LineEnding;
    use crate::search::SearchResult;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_search_result(
        path: &str,
        line_number: usize,
        line: &str,
        replacement: &str,
        included: bool,
        replace_result: Option<ReplaceResult>,
    ) -> SearchResult {
        SearchResult {
            path: PathBuf::from(path),
            line_number,
            line: line.to_string(),
            line_ending: LineEnding::Lf,
            replacement: replacement.to_string(),
            included,
            replace_result,
        }
    }

    #[test]
    fn test_replace_in_file_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "line 1\nold text\nline 3\nold text\nline 5\n";
        std::fs::write(&file_path, content).unwrap();

        // Create search results
        let mut results = vec![
            create_search_result(
                file_path.to_str().unwrap(),
                2,
                "old text",
                "new text",
                true,
                None,
            ),
            create_search_result(
                file_path.to_str().unwrap(),
                4,
                "old text",
                "new text",
                true,
                None,
            ),
        ];

        // Perform replacement
        let result = replace_in_file(&mut results);
        assert!(result.is_ok());

        // Verify replacements were marked as successful
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].replace_result, Some(ReplaceResult::Success));
        assert_eq!(results[1].replace_result, Some(ReplaceResult::Success));

        // Verify file content
        let new_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(new_content, "line 1\nnew text\nline 3\nnew text\nline 5\n");
    }

    #[test]
    fn test_replace_in_file_success_no_final_newline() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "line 1\nold text\nline 3\nold text\nline 5";
        std::fs::write(&file_path, content).unwrap();

        // Create search results
        let mut results = vec![
            create_search_result(
                file_path.to_str().unwrap(),
                2,
                "old text",
                "new text",
                true,
                None,
            ),
            create_search_result(
                file_path.to_str().unwrap(),
                4,
                "old text",
                "new text",
                true,
                None,
            ),
        ];

        // Perform replacement
        let result = replace_in_file(&mut results);
        assert!(result.is_ok());

        // Verify replacements were marked as successful
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].replace_result, Some(ReplaceResult::Success));
        assert_eq!(results[1].replace_result, Some(ReplaceResult::Success));

        // Verify file content
        let new_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(new_content, "line 1\nnew text\nline 3\nnew text\nline 5");
    }

    #[test]
    fn test_replace_in_file_success_windows_newlines() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "line 1\r\nold text\r\nline 3\r\nold text\r\nline 5\r\n";
        std::fs::write(&file_path, content).unwrap();

        // Create search results
        let mut results = vec![
            create_search_result(
                file_path.to_str().unwrap(),
                2,
                "old text",
                "new text",
                true,
                None,
            ),
            create_search_result(
                file_path.to_str().unwrap(),
                4,
                "old text",
                "new text",
                true,
                None,
            ),
        ];

        // Perform replacement
        let result = replace_in_file(&mut results);
        assert!(result.is_ok());

        // Verify replacements were marked as successful
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].replace_result, Some(ReplaceResult::Success));
        assert_eq!(results[1].replace_result, Some(ReplaceResult::Success));

        // Verify file content
        let new_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            new_content,
            "line 1\r\nnew text\r\nline 3\r\nnew text\r\nline 5\r\n"
        );
    }

    #[test]
    fn test_replace_in_file_success_mixed_newlines() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "\n\r\nline 1\nold text\r\nline 3\nline 4\r\nline 5\r\n\n\n";
        std::fs::write(&file_path, content).unwrap();

        // Create search results
        let mut results = vec![
            create_search_result(
                file_path.to_str().unwrap(),
                4,
                "old text",
                "new text",
                true,
                None,
            ),
            create_search_result(
                file_path.to_str().unwrap(),
                7,
                "line 5",
                "updated line 5",
                true,
                None,
            ),
        ];

        // Perform replacement
        let result = replace_in_file(&mut results);
        assert!(result.is_ok());

        // Verify replacements were marked as successful
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].replace_result, Some(ReplaceResult::Success));
        assert_eq!(results[1].replace_result, Some(ReplaceResult::Success));

        // Verify file content
        let new_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            new_content,
            "\n\r\nline 1\nnew text\r\nline 3\nline 4\r\nupdated line 5\r\n\n\n"
        );
    }

    #[test]
    fn test_replace_in_file_line_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "line 1\nactual text\nline 3\n";
        std::fs::write(&file_path, content).unwrap();

        // Create search result with mismatching line
        let mut results = vec![create_search_result(
            file_path.to_str().unwrap(),
            2,
            "expected text",
            "new text",
            true,
            None,
        )];

        // Perform replacement
        let result = replace_in_file(&mut results);
        assert!(result.is_ok());

        // Verify replacement was marked as error
        assert_eq!(
            results[0].replace_result,
            Some(ReplaceResult::Error(
                "File changed since last search".to_owned()
            ))
        );

        // Verify file content is unchanged (except for newlines)
        let new_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(new_content, "line 1\nactual text\nline 3\n");
    }

    #[test]
    fn test_replace_in_file_nonexistent_file() {
        let mut results = vec![create_search_result(
            "/nonexistent/path/file.txt",
            1,
            "old",
            "new",
            true,
            None,
        )];

        let result = replace_in_file(&mut results);
        assert!(result.is_err());
    }

    #[test]
    fn test_replace_in_file_no_parent_directory() {
        let mut results = vec![SearchResult {
            path: PathBuf::from("/"),
            line_number: 0,
            line: "foo".into(),
            line_ending: LineEnding::Lf,
            replacement: "bar".into(),
            included: true,
            replace_result: None,
        }];

        let result = replace_in_file(&mut results);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("no parent directory"));
        }
    }
}
