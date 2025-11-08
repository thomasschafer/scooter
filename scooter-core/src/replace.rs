use frep_core::{
    replace::{ReplaceResult, replace_in_file, replacement_if_match},
    search::{FileSearcher, SearchResultWithReplacement},
};
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

use crate::{
    app::{BackgroundProcessingEvent, Event, EventHandlingResult},
    commands::CommandResults,
};

pub fn split_results(
    results: Vec<SearchResultWithReplacement>,
) -> (Vec<SearchResultWithReplacement>, usize) {
    let (included, excluded): (Vec<_>, Vec<_>) = results
        .into_iter()
        .partition(|res| res.search_result.included);
    let num_ignored = excluded.len();
    (included, num_ignored)
}

fn group_results(
    included: Vec<SearchResultWithReplacement>,
) -> HashMap<Option<PathBuf>, Vec<SearchResultWithReplacement>> {
    let mut path_groups = HashMap::<Option<PathBuf>, Vec<SearchResultWithReplacement>>::new();
    for res in included {
        path_groups
            .entry(res.search_result.path.clone())
            .or_default()
            .push(res);
    }
    path_groups
}

pub fn spawn_replace_included<T: Fn(SearchResultWithReplacement) + Send + Sync + 'static>(
    search_results: Vec<SearchResultWithReplacement>,
    cancelled: Arc<AtomicBool>,
    replacements_completed: Arc<AtomicUsize>,
    validation_search_config: Option<FileSearcher>,
    on_completion: T,
) -> usize {
    let (included, num_ignored) = split_results(search_results);

    thread::spawn(move || {
        let path_groups = group_results(included);

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap();

        pool.install(|| {
            path_groups
                .into_par_iter()
                .for_each(|(_path, mut results)| {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }

                    if let Some(config) = &validation_search_config {
                        validate_search_result_correctness(config, &results);
                    }
                    if let Err(file_err) = replace_in_file(&mut results) {
                        for res in &mut results {
                            res.replace_result = Some(ReplaceResult::Error(file_err.to_string()));
                        }
                    }
                    replacements_completed.fetch_add(results.len(), Ordering::Relaxed);

                    for result in results {
                        on_completion(result);
                    }
                });
        });
    });

    num_ignored
}

fn validate_search_result_correctness(
    validation_search_config: &FileSearcher,
    results: &[SearchResultWithReplacement],
) {
    for res in results {
        let expected = replacement_if_match(
            &res.search_result.line,
            validation_search_config.search(),
            validation_search_config.replace(),
        );
        let actual = &res.replacement;
        assert_eq!(
            expected.as_ref(),
            Some(actual),
            "Expected replacement does not match actual"
        );
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaceState {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: Vec<SearchResultWithReplacement>,
    pub replacement_errors_pos: usize,
}

impl ReplaceState {
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn handle_command_results(&mut self, event: CommandResults) -> EventHandlingResult {
        #[allow(clippy::match_same_arms)]
        match event {
            CommandResults::ScrollErrorsDown => {
                self.scroll_replacement_errors_down();
                EventHandlingResult::Rerender
            }
            CommandResults::ScrollErrorsUp => {
                self.scroll_replacement_errors_up();
                EventHandlingResult::Rerender
            }
            CommandResults::Quit => EventHandlingResult::Exit(None),
        }
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
    pub processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    pub cancelled: Arc<AtomicBool>,
    pub replacement_started: Instant,
    pub num_replacements_completed: Arc<AtomicUsize>,
    pub total_replacements: usize,
}

impl PerformingReplacementState {
    pub fn new(
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
        cancelled: Arc<AtomicBool>,
        num_replacements_completed: Arc<AtomicUsize>,
        total_replacements: usize,
    ) -> Self {
        Self {
            processing_receiver,
            cancelled,
            replacement_started: Instant::now(),
            num_replacements_completed,
            total_replacements,
        }
    }
}

pub fn perform_replacement(
    search_results: Vec<SearchResultWithReplacement>,
    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    cancelled: Arc<AtomicBool>,
    replacements_completed: Arc<AtomicUsize>,
    event_sender: UnboundedSender<Event>,
    validation_search_config: Option<FileSearcher>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        cancelled.store(false, Ordering::Relaxed);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let num_ignored = crate::replace::spawn_replace_included(
            search_results,
            cancelled,
            replacements_completed,
            validation_search_config,
            move |result| {
                let _ = tx.send(result); // Ignore error if receiver is dropped
            },
        );

        let mut rerender_interval = tokio::time::interval(Duration::from_millis(92)); // Slightly random duration so that time taken isn't a round number

        let mut replacement_results = Vec::new();
        loop {
            tokio::select! {
                res = rx.recv() => match res {
                    Some(res) => replacement_results.push(res),
                    None => break,
                },
                _ = rerender_interval.tick() => {
                    let _ = event_sender.send(Event::Rerender);
                }
            }
        }

        let _ = event_sender.send(Event::Rerender);

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use frep_core::{
        line_reader::LineEnding,
        replace::ReplaceResult,
        search::{SearchResult, SearchResultWithReplacement},
    };

    use crate::{
        app::EventHandlingResult,
        commands::CommandResults,
        replace::{self, ReplaceState},
    };

    fn create_search_result_with_replacement(
        path: &str,
        line_number: usize,
        line: &str,
        replacement: &str,
        included: bool,
        replace_result: Option<ReplaceResult>,
    ) -> SearchResultWithReplacement {
        SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from(path)),
                line_number,
                line: line.to_string(),
                line_ending: LineEnding::Lf,
                included,
            },
            replacement: replacement.to_string(),
            replace_result,
        }
    }

    #[test]
    fn test_split_results_all_included() {
        let result1 =
            create_search_result_with_replacement("file1.txt", 1, "line1", "repl1", true, None);
        let result2 =
            create_search_result_with_replacement("file2.txt", 2, "line2", "repl2", true, None);
        let result3 =
            create_search_result_with_replacement("file3.txt", 3, "line3", "repl3", true, None);

        let search_results = vec![result1.clone(), result2.clone(), result3.clone()];

        let (included, num_ignored) = replace::split_results(search_results);
        assert_eq!(num_ignored, 0);
        assert_eq!(included, vec![result1, result2, result3]);
    }

    #[test]
    fn test_split_results_mixed() {
        let result1 =
            create_search_result_with_replacement("file1.txt", 1, "line1", "repl1", true, None);
        let result2 =
            create_search_result_with_replacement("file2.txt", 2, "line2", "repl2", false, None);
        let result3 =
            create_search_result_with_replacement("file3.txt", 3, "line3", "repl3", true, None);
        let result4 =
            create_search_result_with_replacement("file4.txt", 4, "line4", "repl4", false, None);

        let search_results = vec![result1.clone(), result2, result3.clone(), result4];

        let (included, num_ignored) = replace::split_results(search_results);
        assert_eq!(num_ignored, 2);
        assert_eq!(included, vec![result1, result3]);
        assert!(included.iter().all(|r| r.search_result.included));
    }

    #[test]
    fn test_replace_state_scroll_replacement_errors_up() {
        let mut state = ReplaceState {
            num_successes: 5,
            num_ignored: 2,
            errors: vec![
                create_search_result_with_replacement(
                    "file1.txt",
                    1,
                    "error1",
                    "repl1",
                    true,
                    Some(ReplaceResult::Error("err1".to_string())),
                ),
                create_search_result_with_replacement(
                    "file2.txt",
                    2,
                    "error2",
                    "repl2",
                    true,
                    Some(ReplaceResult::Error("err2".to_string())),
                ),
                create_search_result_with_replacement(
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
                create_search_result_with_replacement(
                    "file1.txt",
                    1,
                    "error1",
                    "repl1",
                    true,
                    Some(ReplaceResult::Error("err1".to_string())),
                ),
                create_search_result_with_replacement(
                    "file2.txt",
                    2,
                    "error2",
                    "repl2",
                    true,
                    Some(ReplaceResult::Error("err2".to_string())),
                ),
                create_search_result_with_replacement(
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
    fn test_replace_state_handle_command_results() {
        let mut state = ReplaceState {
            num_successes: 5,
            num_ignored: 2,
            errors: vec![
                create_search_result_with_replacement(
                    "file1.txt",
                    1,
                    "error1",
                    "repl1",
                    true,
                    Some(ReplaceResult::Error("err1".to_string())),
                ),
                create_search_result_with_replacement(
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

        let result = state.handle_command_results(CommandResults::ScrollErrorsDown);
        assert!(matches!(result, EventHandlingResult::Rerender));
        assert_eq!(state.replacement_errors_pos, 1);

        let result = state.handle_command_results(CommandResults::ScrollErrorsUp);
        assert!(matches!(result, EventHandlingResult::Rerender));
        assert_eq!(state.replacement_errors_pos, 0);

        let result = state.handle_command_results(CommandResults::Quit);
        assert!(matches!(result, EventHandlingResult::Exit(None)));
    }

    #[test]
    fn test_calculate_statistics_all_success() {
        let results = vec![
            create_search_result_with_replacement(
                "file1.txt",
                1,
                "line1",
                "repl1",
                true,
                Some(ReplaceResult::Success),
            ),
            create_search_result_with_replacement(
                "file2.txt",
                2,
                "line2",
                "repl2",
                true,
                Some(ReplaceResult::Success),
            ),
            create_search_result_with_replacement(
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
        let error_result = create_search_result_with_replacement(
            "file2.txt",
            2,
            "line2",
            "repl2",
            true,
            Some(ReplaceResult::Error("test error".to_string())),
        );
        let results = vec![
            create_search_result_with_replacement(
                "file1.txt",
                1,
                "line1",
                "repl1",
                true,
                Some(ReplaceResult::Success),
            ),
            error_result.clone(),
            create_search_result_with_replacement(
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
        assert_eq!(
            stats.errors[0].search_result.path,
            error_result.search_result.path
        );
    }

    #[test]
    fn test_calculate_statistics_with_none_results() {
        let results = vec![
            create_search_result_with_replacement(
                "file1.txt",
                1,
                "line1",
                "repl1",
                true,
                Some(ReplaceResult::Success),
            ),
            create_search_result_with_replacement("file2.txt", 2, "line2", "repl2", true, None), // This should be treated as an error
            create_search_result_with_replacement(
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
        assert_eq!(
            stats.errors[0].search_result.path,
            Some(PathBuf::from("file2.txt"))
        );
        assert_eq!(
            stats.errors[0].replace_result,
            Some(ReplaceResult::Error(
                "Failed to find search result in file".to_owned()
            ))
        );
    }
}
