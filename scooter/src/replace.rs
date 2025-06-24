use crossterm::event::KeyEvent;
use crossterm::style::Stylize as _;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

use frep_core::search::SearchResult;

use crate::app::{AppEvent, BackgroundProcessingEvent, Event, EventHandlingResult};

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
            (KeyCode::Enter | KeyCode::Char('q'), _) => return EventHandlingResult::Exit(None),
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
    pub handle: JoinHandle<()>,
    #[allow(dead_code)]
    pub processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    pub processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    pub cancelled: Arc<AtomicBool>,
    pub replacement_started: Instant,
    pub num_replacements_completed: Arc<AtomicUsize>,
    pub total_replacements: usize,
}

impl PerformingReplacementState {
    pub fn new(
        handle: JoinHandle<()>,
        processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
        cancelled: Arc<AtomicBool>,
        num_replacements_completed: Arc<AtomicUsize>,
        total_replacements: usize,
    ) -> Self {
        Self {
            handle,
            processing_sender,
            processing_receiver,
            cancelled,
            replacement_started: Instant::now(),
            num_replacements_completed,
            total_replacements,
        }
    }
}

pub fn perform_replacement(
    search_results: Vec<SearchResult>,
    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    cancelled: Arc<AtomicBool>,
    replacements_completed: Arc<AtomicUsize>,
    event_sender: UnboundedSender<Event>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        cancelled.store(false, Ordering::Relaxed);

        let (mut replacements_handle, num_ignored) = scooter_core::replace::spawn_replace_included(
            search_results,
            cancelled,
            replacements_completed,
        );

        let mut rerender_interval = tokio::time::interval(Duration::from_millis(92)); // Slightly random duration so that time taken isn't a round number

        let mut replacement_results = Vec::new();
        loop {
            tokio::select! {
                res = replacements_handle.recv() => match res {
                    Some(res) => replacement_results.push(res),
                    None => break,
                },
                _ = rerender_interval.tick() => {
                    let _ = event_sender.send(Event::App(AppEvent::Rerender));
                }
            }
        }

        let _ = event_sender.send(Event::App(AppEvent::Rerender));

        let stats = frep_core::replace::calculate_statistics(replacement_results);
        // Ignore error: we may have gone back to the previous screen
        let _ = background_processing_sender.send(BackgroundProcessingEvent::ReplacementCompleted(
            ReplaceState {
                num_successes: stats.num_successes,
                num_ignored,
                errors: stats.errors,
                replacement_errors_pos: 0,
            },
        ));
    })
}

pub fn format_replacement_results(
    num_successes: usize,
    num_ignored: Option<usize>,
    errors: Option<&[SearchResult]>,
) -> String {
    let errors_display = if let Some(errors) = errors {
        #[allow(clippy::format_collect)]
        errors
            .iter()
            .map(|error| {
                let (path, error) = error.display_error();
                format!("\n{path}:\n  {}", error.red())
            })
            .collect::<String>()
    } else {
        String::new()
    };

    let maybe_ignored_str = match num_ignored {
        Some(n) => format!("\nIgnored (lines): {n}"),
        None => "".into(),
    };
    let maybe_errors_str = match errors {
        Some(errors) => format!(
            "\nErrors: {num_errors}{errors_display}",
            num_errors = errors.len()
        ),
        None => "".into(),
    };

    format!("Successful replacements (lines): {num_successes}{maybe_ignored_str}{maybe_errors_str}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use frep_core::{line_reader::LineEnding, replace::ReplaceResult};
    use std::path::PathBuf;

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
        assert_eq!(result, EventHandlingResult::Exit(None));

        // Test exit with 'q'
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::Exit(None));

        // Test unhandled key
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        let result = state.handle_key_results(&key);
        assert_eq!(result, EventHandlingResult::None);
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

        let stats = frep_core::replace::calculate_statistics(results);
        assert_eq!(stats.num_successes, 3);
        assert_eq!(stats.errors.len(), 0);
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

        let stats = frep_core::replace::calculate_statistics(results);
        assert_eq!(stats.num_successes, 2);
        assert_eq!(stats.errors.len(), 1);
        assert_eq!(stats.errors[0].path, error_result.path);
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

        let stats = frep_core::replace::calculate_statistics(results);
        assert_eq!(stats.num_successes, 2);
        assert_eq!(stats.errors.len(), 1);
        assert_eq!(stats.errors[0].path, PathBuf::from("file2.txt"));
        assert_eq!(
            stats.errors[0].replace_result,
            Some(ReplaceResult::Error(
                "Failed to find search result in file".to_owned()
            ))
        );
    }

    #[test]
    fn test_format_replacement_results_no_errors() {
        let result = format_replacement_results(5, Some(2), Some(&[]));
        assert_eq!(
            result,
            "Successful replacements (lines): 5\nIgnored (lines): 2\nErrors: 0"
        );
    }

    #[test]
    fn test_format_replacement_results_with_errors() {
        let error_result = create_search_result(
            "file.txt",
            10,
            "line",
            "replacement",
            true,
            Some(ReplaceResult::Error("Test error".to_string())),
        );

        let result = format_replacement_results(3, Some(1), Some(&[error_result]));
        assert!(result.contains("Successful replacements (lines): 3"));
        assert!(result.contains("Ignored (lines): 1"));
        assert!(result.contains("Errors: 1"));
        assert!(result.contains("file.txt:10"));
        assert!(result.contains("Test error"));
    }

    #[test]
    fn test_format_replacement_results_no_ignored_count() {
        let result = format_replacement_results(7, None, Some(&[]));
        assert_eq!(result, "Successful replacements (lines): 7\nErrors: 0");
        assert!(!result.contains("Ignored (lines):"));
    }
}
