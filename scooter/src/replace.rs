use crossterm::event::KeyEvent;
use futures::future;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tempfile::NamedTempFile;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        Semaphore,
    },
    task::JoinHandle,
};

use crate::{
    app::{BackgroundProcessingEvent, EventHandlingResult},
    search::SearchResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceResult {
    Success,
    Error(String),
}

#[derive(Debug, Eq, PartialEq)]
pub struct ReplaceState {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: Vec<SearchResult>,
    pub replacement_errors_pos: usize,
}

impl ReplaceState {
    pub fn handle_key_results(&mut self, key: &KeyEvent) -> EventHandlingResult {
        #[allow(clippy::match_same_arms)]
        match (key.code, key.modifiers) {
            (KeyCode::Char('j') | KeyCode::Down, _)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.scroll_replacement_errors_down();
            }
            (KeyCode::Char('k') | KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.scroll_replacement_errors_up();
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {} // TODO: scroll down half a page
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {} // TODO: scroll down a full page
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {} // TODO: scroll up half a page
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {} // TODO: scroll up a full page
            (KeyCode::Enter | KeyCode::Char('q'), _) => return EventHandlingResult::Exit,
            _ => return EventHandlingResult::None,
        }
        EventHandlingResult::Rerender
    }

    pub fn scroll_replacement_errors_up(&mut self) {
        if self.replacement_errors_pos == 0 {
            self.replacement_errors_pos = self.errors.len();
        }
        self.replacement_errors_pos = self.replacement_errors_pos.saturating_sub(1);
    }

    pub fn scroll_replacement_errors_down(&mut self) {
        if self.replacement_errors_pos >= self.errors.len().saturating_sub(1) {
            self.replacement_errors_pos = 0;
        } else {
            self.replacement_errors_pos += 1;
        }
    }
}

#[derive(Debug)]
pub struct PerformingReplacementState {
    pub handle: Option<JoinHandle<()>>,
    #[allow(dead_code)]
    pub processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    pub processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    pub cancelled: Arc<AtomicBool>,
}

impl PerformingReplacementState {
    pub fn new(
        handle: Option<JoinHandle<()>>,
        processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            handle,
            processing_sender,
            processing_receiver,
            cancelled,
        }
    }

    pub fn set_handle(&mut self, handle: JoinHandle<()>) {
        self.handle = Some(handle);
    }
}

pub fn perform_replacement(
    search_state: crate::app::SearchState,
    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    cancelled: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        cancelled.store(false, Ordering::Relaxed);

        let mut path_groups: HashMap<PathBuf, Vec<SearchResult>> = HashMap::new();
        let (included, num_ignored) = split_results(search_state.results);
        for res in included {
            path_groups.entry(res.path.clone()).or_default().push(res);
        }

        let semaphore = Arc::new(Semaphore::new(8));
        let mut file_tasks = vec![];

        for (path, mut results) in path_groups {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }

            let semaphore = semaphore.clone();
            let task = tokio::spawn(async move {
                let permit = semaphore.clone().acquire_owned().await.unwrap();
                if let Err(file_err) = replace_in_file(path, &mut results).await {
                    for res in &mut results {
                        res.replace_result = Some(ReplaceResult::Error(file_err.to_string()));
                    }
                }
                drop(permit);
                results
            });
            file_tasks.push(task);
        }

        let replacement_results = future::join_all(file_tasks)
            .await
            .into_iter()
            .flat_map(Result::unwrap);
        let replace_state = calculate_statistics(replacement_results, num_ignored);

        // Ignore error: we may have gone back to the previous screen
        let _ = background_processing_sender.send(BackgroundProcessingEvent::ReplacementCompleted(
            replace_state,
        ));
    })
}

pub fn calculate_statistics<I>(results: I, num_ignored: usize) -> ReplaceState
where
    I: IntoIterator<Item = SearchResult>,
{
    let mut num_successes = 0;
    let mut errors = vec![];

    results.into_iter().for_each(|res| {
        assert!(
            res.included,
            "Expected only included results, found {res:?}"
        );
        match &res.replace_result {
            Some(ReplaceResult::Success) => {
                num_successes += 1;
            }
            None => {
                let mut res = res.clone();
                res.replace_result = Some(ReplaceResult::Error(
                    "Failed to find search result in file".to_owned(),
                ));
                errors.push(res);
            }
            Some(ReplaceResult::Error(_)) => {
                errors.push(res.clone());
            }
        }
    });

    ReplaceState {
        num_successes,
        num_ignored,
        errors,
        replacement_errors_pos: 0,
    }
}

async fn replace_in_file(file_path: PathBuf, results: &mut [SearchResult]) -> anyhow::Result<()> {
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
        let input = File::open(&file_path).await?;
        let reader = BufReader::new(input);

        let output = File::create(temp_output_file.path()).await?;
        let mut writer = BufWriter::new(output);

        let mut lines = reader.lines();
        let mut line_number = 0;
        while let Some(mut line) = lines.next_line().await? {
            if let Some(res) = line_map.get_mut(&(line_number + 1)) {
                if line == res.line {
                    line.clone_from(&res.replacement);
                    res.replace_result = Some(ReplaceResult::Success);
                } else {
                    res.replace_result = Some(ReplaceResult::Error(
                        "File changed since last search".to_owned(),
                    ));
                }
            }
            line.push('\n');
            writer.write_all(line.as_bytes()).await?;
            line_number += 1;
        }

        writer.flush().await?;
    }

    temp_output_file.persist(&file_path)?;
    Ok(())
}

pub fn split_results(results: Vec<SearchResult>) -> (Vec<SearchResult>, usize) {
    let (included, excluded): (Vec<_>, Vec<_>) = results.into_iter().partition(|res| res.included);
    let num_ignored = excluded.len();
    (included, num_ignored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::fs;

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
            replacement: replacement.to_string(),
            included,
            replace_result,
        }
    }

    #[test]
    fn test_replace_state_scroll_replacement_errors_up() {
        let mut state = ReplaceState {
            num_successes: 5,
            num_ignored: 2,
            errors: vec![
                create_search_result(
                    "file1.txt",
                    1,
                    "error1",
                    "repl1",
                    true,
                    Some(ReplaceResult::Error("err1".to_string())),
                ),
                create_search_result(
                    "file2.txt",
                    2,
                    "error2",
                    "repl2",
                    true,
                    Some(ReplaceResult::Error("err2".to_string())),
                ),
                create_search_result(
                    "file3.txt",
                    3,
                    "error3",
                    "repl3",
                    true,
                    Some(ReplaceResult::Error("err3".to_string())),
                ),
            ],
            replacement_errors_pos: 1,
        };

        state.scroll_replacement_errors_up();
        assert_eq!(state.replacement_errors_pos, 0);

        state.scroll_replacement_errors_up();
        assert_eq!(state.replacement_errors_pos, 2);

        state.scroll_replacement_errors_up();
        assert_eq!(state.replacement_errors_pos, 1);
    }

    #[test]
    fn test_replace_state_scroll_replacement_errors_down() {
        let mut state = ReplaceState {
            num_successes: 5,
            num_ignored: 2,
            errors: vec![
                create_search_result(
                    "file1.txt",
                    1,
                    "error1",
                    "repl1",
                    true,
                    Some(ReplaceResult::Error("err1".to_string())),
                ),
                create_search_result(
                    "file2.txt",
                    2,
                    "error2",
                    "repl2",
                    true,
                    Some(ReplaceResult::Error("err2".to_string())),
                ),
                create_search_result(
                    "file3.txt",
                    3,
                    "error3",
                    "repl3",
                    true,
                    Some(ReplaceResult::Error("err3".to_string())),
                ),
            ],
            replacement_errors_pos: 1,
        };

        state.scroll_replacement_errors_down();
        assert_eq!(state.replacement_errors_pos, 2);

        state.scroll_replacement_errors_down();
        assert_eq!(state.replacement_errors_pos, 0);

        state.scroll_replacement_errors_down();
        assert_eq!(state.replacement_errors_pos, 1);
    }

    #[test]
    fn test_replace_state_handle_key_results() {
        let mut state = ReplaceState {
            num_successes: 5,
            num_ignored: 2,
            errors: vec![
                create_search_result(
                    "file1.txt",
                    1,
                    "error1",
                    "repl1",
                    true,
                    Some(ReplaceResult::Error("err1".to_string())),
                ),
                create_search_result(
                    "file2.txt",
                    2,
                    "error2",
                    "repl2",
                    true,
                    Some(ReplaceResult::Error("err2".to_string())),
                ),
            ],
            replacement_errors_pos: 0,
        };

        // Test scrolling down with 'j'
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::Rerender);
        assert_eq!(state.replacement_errors_pos, 1);

        // Test scrolling up with 'k'
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::Rerender);
        assert_eq!(state.replacement_errors_pos, 0);

        // Test scrolling down with Down arrow
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::Rerender);
        assert_eq!(state.replacement_errors_pos, 1);

        // Test exit with Enter
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::Exit);

        // Test exit with 'q'
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::Exit);

        // Test unhandled key
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::None);
    }

    #[test]
    fn test_split_results_all_included() {
        let result1 = create_search_result("file1.txt", 1, "line1", "repl1", true, None);
        let result2 = create_search_result("file2.txt", 2, "line2", "repl2", true, None);
        let result3 = create_search_result("file3.txt", 3, "line3", "repl3", true, None);

        let search_results = vec![result1.clone(), result2.clone(), result3.clone()];

        let (included, num_ignored) = split_results(search_results);
        assert_eq!(num_ignored, 0);
        assert_eq!(included, vec![result1, result2, result3]);
    }

    #[test]
    fn test_split_results_mixed() {
        let result1 = create_search_result("file1.txt", 1, "line1", "repl1", true, None);
        let result2 = create_search_result("file2.txt", 2, "line2", "repl2", false, None);
        let result3 = create_search_result("file3.txt", 3, "line3", "repl3", true, None);
        let result4 = create_search_result("file4.txt", 4, "line4", "repl4", false, None);

        let search_results = vec![result1.clone(), result2, result3.clone(), result4];

        let (included, num_ignored) = split_results(search_results);
        assert_eq!(num_ignored, 2);
        assert_eq!(included, vec![result1, result3]);
        assert!(included.iter().all(|r| r.included));
    }

    #[test]
    fn test_calculate_statistics_all_success() {
        let results = vec![
            create_search_result(
                "file1.txt",
                1,
                "line1",
                "repl1",
                true,
                Some(ReplaceResult::Success),
            ),
            create_search_result(
                "file2.txt",
                2,
                "line2",
                "repl2",
                true,
                Some(ReplaceResult::Success),
            ),
            create_search_result(
                "file3.txt",
                3,
                "line3",
                "repl3",
                true,
                Some(ReplaceResult::Success),
            ),
        ];

        let stats = calculate_statistics(results, 0);
        assert_eq!(stats.num_successes, 3);
        assert_eq!(stats.num_ignored, 0);
        assert_eq!(stats.errors.len(), 0);
        assert_eq!(stats.replacement_errors_pos, 0);
    }

    #[test]
    fn test_calculate_statistics_with_errors() {
        let error_result = create_search_result(
            "file2.txt",
            2,
            "line2",
            "repl2",
            true,
            Some(ReplaceResult::Error("test error".to_string())),
        );
        let results = vec![
            create_search_result(
                "file1.txt",
                1,
                "line1",
                "repl1",
                true,
                Some(ReplaceResult::Success),
            ),
            error_result.clone(),
            create_search_result(
                "file3.txt",
                3,
                "line3",
                "repl3",
                true,
                Some(ReplaceResult::Success),
            ),
        ];

        let stats = calculate_statistics(results, 1);
        assert_eq!(stats.num_successes, 2);
        assert_eq!(stats.num_ignored, 1);
        assert_eq!(stats.errors.len(), 1);
        assert_eq!(stats.errors[0].path, error_result.path);
        assert_eq!(stats.replacement_errors_pos, 0);
    }

    #[test]
    fn test_calculate_statistics_with_none_results() {
        let results = vec![
            create_search_result(
                "file1.txt",
                1,
                "line1",
                "repl1",
                true,
                Some(ReplaceResult::Success),
            ),
            create_search_result("file2.txt", 2, "line2", "repl2", true, None), // This should be treated as an error
            create_search_result(
                "file3.txt",
                3,
                "line3",
                "repl3",
                true,
                Some(ReplaceResult::Success),
            ),
        ];

        let stats = calculate_statistics(results, 0);
        assert_eq!(stats.num_successes, 2);
        assert_eq!(stats.num_ignored, 0);
        assert_eq!(stats.errors.len(), 1);
        assert_eq!(stats.errors[0].path, PathBuf::from("file2.txt"));
        assert_eq!(
            stats.errors[0].replace_result,
            Some(ReplaceResult::Error(
                "Failed to find search result in file".to_owned()
            ))
        );
    }

    #[tokio::test]
    async fn test_replace_in_file_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "line 1\nold text\nline 3\nold text\nline 5";
        fs::write(&file_path, content).await.unwrap();

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
        let result = replace_in_file(file_path.clone(), &mut results).await;
        assert!(result.is_ok());

        // Verify replacements were marked as successful
        assert_eq!(results[0].replace_result, Some(ReplaceResult::Success));
        assert_eq!(results[1].replace_result, Some(ReplaceResult::Success));

        // Verify file content
        let new_content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(new_content, "line 1\nnew text\nline 3\nnew text\nline 5\n");
    }

    #[tokio::test]
    async fn test_replace_in_file_line_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Create test file
        let content = "line 1\nactual text\nline 3";
        fs::write(&file_path, content).await.unwrap();

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
        let result = replace_in_file(file_path.clone(), &mut results).await;
        assert!(result.is_ok());

        // Verify replacement was marked as error
        assert_eq!(
            results[0].replace_result,
            Some(ReplaceResult::Error(
                "File changed since last search".to_owned()
            ))
        );

        // Verify file content is unchanged (except for newlines)
        let new_content = fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(new_content, "line 1\nactual text\nline 3\n");
    }

    #[tokio::test]
    async fn test_replace_in_file_nonexistent_file() {
        let mut results = vec![create_search_result(
            "/nonexistent/path/file.txt",
            1,
            "old",
            "new",
            true,
            None,
        )];

        let result =
            replace_in_file(PathBuf::from("/nonexistent/path/file.txt"), &mut results).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_replace_in_file_no_parent_directory() {
        let mut results = vec![];

        // PathBuf::from("/") has no parent
        let result = replace_in_file(PathBuf::from("/"), &mut results).await;
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("no parent directory"));
        }
    }
}
