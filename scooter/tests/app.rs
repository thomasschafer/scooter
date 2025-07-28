use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use frep_core::line_reader::LineEnding;
use frep_core::replace::ReplaceResult;
use frep_core::search::SearchResult;
use frep_core::search::SearchResultWithReplacement;
use insta::assert_debug_snapshot;
use scooter::app::EventHandlingResult;
use scooter::app::FocussedSection;
use scooter::app::Popup;
use scooter::app::Screen;
use scooter::app::SearchFieldsState;
use serial_test::serial;
use std::env::current_dir;
use std::fs;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, Arc};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::{cmp::max, sync::atomic::AtomicUsize};
use tempfile::TempDir;
use tokio::sync::mpsc;

use scooter::{
    app::{App, AppError, AppRunConfig, SearchState},
    fields::{FieldValue, SearchFieldValues, SearchFields},
    replace::{PerformingReplacementState, ReplaceState},
    test_with_both_regex_modes,
};

mod utils;

#[tokio::test]
async fn test_replace_state() {
    let mut state = ReplaceState {
        num_successes: 2,
        num_ignored: 1,
        errors: (1..3)
            .map(|n| SearchResultWithReplacement {
                search_result: SearchResult {
                    path: PathBuf::from(format!("error-{n}.txt")),
                    line_number: 1,
                    line: format!("line {n}"),
                    line_ending: LineEnding::Lf,
                    included: true,
                },
                replacement: format!("error replacement {n}"),
                replace_result: Some(ReplaceResult::Error(format!("Test error {n}"))),
            })
            .collect::<Vec<_>>(),
        replacement_errors_pos: 0,
    };

    state.scroll_replacement_errors_down();
    assert_eq!(state.replacement_errors_pos, 1);
    state.scroll_replacement_errors_down();
    assert_eq!(state.replacement_errors_pos, 0);
    state.scroll_replacement_errors_up();
    assert_eq!(state.replacement_errors_pos, 1);
    state.scroll_replacement_errors_up();
    assert_eq!(state.replacement_errors_pos, 0);
}

#[tokio::test]
async fn test_app_reset() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );
    app.current_screen = Screen::Results(ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    });

    app.reset();

    assert!(matches!(app.current_screen, Screen::SearchFields(_)));
}

#[tokio::test]
async fn test_back_from_results() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );
    let (sender, receiver) = mpsc::unbounded_channel();
    app.current_screen = Screen::SearchFields(SearchFieldsState {
        focussed_section: FocussedSection::SearchResults,
        search_state: Some(SearchState::new(
            sender,
            receiver,
            Arc::new(AtomicBool::new(false)),
        )),
        search_debounce_timer: None,
        preview_update_state: None,
    });
    app.search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new("foo", false),
            replace: FieldValue::new("bar", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("pattern", false),
            exclude_files: FieldValue::new("", false),
        },
        true,
    );

    let res = app.handle_key_event(&KeyEvent {
        code: KeyCode::Char('o'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res != EventHandlingResult::Exit(None));
    assert_eq!(app.search_fields.search().text(), "foo");
    assert_eq!(app.search_fields.replace().text(), "bar");
    assert!(app.search_fields.fixed_strings().checked);
    assert_eq!(app.search_fields.include_files().text(), "pattern");
    assert_eq!(app.search_fields.exclude_files().text(), "");
    assert!(matches!(app.current_screen, Screen::SearchFields(_)));
}

fn test_error_popup_invalid_input_impl(search_fields: &SearchFieldValues<'_>) {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        search_fields,
        &AppRunConfig::default(),
    );

    // Simulate search being triggered in background
    let res = app.perform_search_if_valid();
    assert!(app.popup().is_none());

    // Hitting enter should show popup
    app.handle_key_event(&KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res != EventHandlingResult::Exit(None));
    assert!(matches!(app.current_screen, Screen::SearchFields(_)));
    assert!(matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(&KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res != EventHandlingResult::Exit(None));
    assert!(!matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(&KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert_eq!(res, EventHandlingResult::Exit(None));
}

#[tokio::test]
async fn test_error_popup_invalid_search() {
    test_error_popup_invalid_input_impl(&SearchFieldValues {
        search: FieldValue::new("search invalid regex(", false),
        replace: FieldValue::new("replacement", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("", false),
    });
}

#[tokio::test]
async fn test_error_popup_invalid_include_files() {
    test_error_popup_invalid_input_impl(&SearchFieldValues {
        search: FieldValue::new("search", false),
        replace: FieldValue::new("replacement", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("foo{", false),
        exclude_files: FieldValue::new("", false),
    });
}

#[tokio::test]
async fn test_error_popup_invalid_exclude_files() {
    test_error_popup_invalid_input_impl(&SearchFieldValues {
        search: FieldValue::new("search", false),
        replace: FieldValue::new("replacement", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("bar{", false),
    });
}

fn test_help_popup_on_screen(initial_screen: Screen) {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );
    let screen_variant = std::mem::discriminant(&initial_screen);
    app.current_screen = initial_screen;

    assert!(app.popup().is_none());
    assert_eq!(mem::discriminant(&app.current_screen), screen_variant);

    let res_open = app.handle_key_event(&KeyEvent {
        code: KeyCode::Char('h'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res_open == EventHandlingResult::Rerender);
    assert!(matches!(app.popup(), Some(Popup::Help)));
    assert_eq!(std::mem::discriminant(&app.current_screen), screen_variant);

    let res_close = app.handle_key_event(&KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res_close == EventHandlingResult::Rerender);
    assert!(app.popup().is_none());
    assert_eq!(std::mem::discriminant(&app.current_screen), screen_variant);
}

#[tokio::test]
async fn test_help_popup_on_search_fields() {
    test_help_popup_on_screen(Screen::SearchFields(SearchFieldsState::default()));
}

#[tokio::test]
async fn test_help_popup_on_search_results() {
    let (sender, receiver) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let initial_screen = Screen::SearchFields(SearchFieldsState {
        focussed_section: FocussedSection::SearchResults,
        search_state: Some(SearchState::new(sender, receiver, cancelled)),
        search_debounce_timer: None,
        preview_update_state: None,
    });
    test_help_popup_on_screen(initial_screen);
}

#[tokio::test]
async fn test_help_popup_on_performing_replacement() {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let initial_screen = Screen::PerformingReplacement(PerformingReplacementState::new(
        receiver,
        cancelled,
        Arc::new(AtomicUsize::new(0)),
        0,
    ));
    test_help_popup_on_screen(initial_screen);
}

#[tokio::test]
async fn test_help_popup_on_results() {
    let results_state = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    };
    test_help_popup_on_screen(Screen::Results(results_state));
}

pub fn wait_until<F>(condition: F, timeout: Duration) -> bool
where
    F: Fn() -> bool,
{
    let start = Instant::now();
    let sleep_duration = max(timeout / 50, Duration::from_millis(1));
    while !condition() && start.elapsed() <= timeout {
        sleep(sleep_duration);
    }
    condition()
}

async fn process_bp_events(app: &mut App) {
    let timeout = Duration::from_secs(5);

    // TODO: will this channel ever close? Should we change testing strategy?
    let timeout_res = tokio::time::timeout(timeout, async {
        while let Some(event) = app.background_processing_recv().await {
            app.handle_background_processing_event(event);
        }
    })
    .await;
    assert!(
        timeout_res.is_ok(),
        "Couldn't process background events in a reasonable time"
    );
}

macro_rules! wait_for_screen {
    ($app:expr, $variant:path) => {
        wait_until(
            || matches!($app.current_screen, $variant(_)),
            Duration::from_secs(1),
        )
    };
}

fn setup_app(
    temp_dir: &TempDir,
    search_field_values: &SearchFieldValues<'_>,
    app_run_config: &AppRunConfig,
) -> App {
    let (app, _app_event_receiver) = App::new_with_receiver(
        temp_dir.path().to_path_buf(),
        search_field_values,
        app_run_config,
    );
    app
}

// TODO(autosave): move these to app_runner tests
// TODO: simplify this test - it is somewhat tied to the current implementation
async fn search_and_replace_test(
    temp_dir: &TempDir,
    search_field_values: &SearchFieldValues<'_>,
    app_run_config: &AppRunConfig,
    expected_matches: Vec<(&Path, usize)>,
) {
    let total_num_expected_matches = expected_matches
        .iter()
        .map(|(_, count)| count)
        .sum::<usize>();

    let mut app = setup_app(temp_dir, search_field_values, app_run_config);
    assert_eq!(app.errors(), vec![]);
    let res = app.perform_search_if_valid();
    assert!(res != EventHandlingResult::Exit(None));

    process_bp_events(&mut app).await;
    assert!(wait_until_search_complete(&app));

    let Screen::SearchFields(SearchFieldsState {
        search_state: Some(state),
        ..
    }) = &mut app.current_screen
    else {
        panic!(
            "Expected SearchComplete results with Some search state, found {:?}",
            app.current_screen
        );
    };

    for (file_path, num_expected_matches) in &expected_matches {
        let num_actual_matches = state
            .results
            .iter()
            .filter(|result| {
                let result_path = result.search_result.path.to_str().unwrap();
                let file_path = file_path.to_str().unwrap();
                result_path == temp_dir.path().join(file_path).to_string_lossy()
            })
            .count();
        let num_expected_matches = *num_expected_matches;
        assert_eq!(
            num_actual_matches,
            num_expected_matches,
            "{}: expected {num_expected_matches}, found {num_actual_matches}",
            file_path.display(),
        );
    }

    assert_eq!(state.results.len(), total_num_expected_matches);

    app.trigger_replacement();

    process_bp_events(&mut app).await;
    assert!(wait_for_screen!(&app, Screen::Results));

    if let Screen::Results(search_state) = &app.current_screen {
        assert_eq!(search_state.num_successes, total_num_expected_matches);
        assert_eq!(search_state.num_ignored, 0);
        assert_eq!(search_state.errors.len(), 0);
    } else {
        panic!(
            "Expected screen to be Screen::Results, instead found {:?}",
            app.current_screen
        );
    }
}

fn wait_until_search_complete(app: &App) -> bool {
    wait_until(
        || {
            if let Screen::SearchFields(ref state) = app.current_screen {
                state
                    .search_state
                    .as_ref()
                    .is_some_and(SearchState::search_has_completed)
            } else {
                false
            }
        },
        Duration::from_secs(1),
    )
}

test_with_both_regex_modes!(
    test_search_replace_defaults,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("t[esES]+t", false),
            replace: FieldValue::new("123,", false),
            ..SearchFieldValues::default()
        };
        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (Path::new("file1.txt"), 5),
                (Path::new("file2.txt"), 2),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a 123, file",
                "It contains some 123, content",
                "For TESTING purposes",
                "Test TEST tEsT tesT 123,",
                "TestbTESTctEsTdtesTe123,",
                " 123, ",
            ),
            "file2.txt" => text!(
                "Another 123, file",
                "With different content",
                "Also for 123,ing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "123,-TEST-123,",
                "something",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_search_replace_fixed_string,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new(".*", false),
            replace: FieldValue::new("example", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (Path::new("file1.txt"), 0),
                (Path::new("file2.txt"), 0),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+examplebar)(baz 456",
                "something",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_search_replace_match_case,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("test", false),
            replace: FieldValue::new("REPLACEMENT", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (Path::new("file1.txt"), 5),
                (Path::new("file2.txt"), 2),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a REPLACEMENT file",
                "It contains some REPLACEMENT content",
                "For TESTING purposes",
                "Test TEST tEsT tesT REPLACEMENT",
                "TestbTESTctEsTdtesTeREPLACEMENT",
                " REPLACEMENT ",
            ),
            "file2.txt" => text!(
                "Another REPLACEMENT file",
                "With different content",
                "Also for REPLACEMENTing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "REPLACEMENT-TEST-tESt",
                "something",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_search_replace_dont_match_case,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("test", false),
            replace: FieldValue::new("REPLACEMENT", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(false, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (Path::new("file1.txt"), 6),
                (Path::new("file2.txt"), 2),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a REPLACEMENT file",
                "It contains some REPLACEMENT content",
                "For REPLACEMENTING purposes",
                "REPLACEMENT REPLACEMENT REPLACEMENT REPLACEMENT REPLACEMENT",
                "REPLACEMENTbREPLACEMENTcREPLACEMENTdREPLACEMENTeREPLACEMENT",
                " REPLACEMENT ",
            ),
            "file2.txt" => text!(
                "Another REPLACEMENT file",
                "With different content",
                "Also for REPLACEMENTing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "REPLACEMENT-REPLACEMENT-REPLACEMENT",
                "something",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_regex,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new(r"\b\w+ing\b", false),
            replace: FieldValue::new("VERB", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (Path::new("file1.txt"), 1),
                (Path::new("file2.txt"), 1),
                (Path::new("file3.txt"), 2),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For VERB purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for VERB",
            ),
            "file3.txt" => text!(
                "VERB",
                "123 bar[a-b]+.*bar)(baz 456",
                "VERB",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_no_matches,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("nonexistent-string", false),
            replace: FieldValue::new("replacement", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (Path::new("file1.txt"), 0),
                (Path::new("file2.txt"), 0),
                (Path::new("file3.txt"), 0),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_invalid_regex,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("[invalid regex", false),
            replace: FieldValue::new("replacement", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        let mut app = setup_app(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
        );

        let res = app.perform_search_if_valid();
        assert!(res != EventHandlingResult::Exit(None));
        assert!(matches!(app.current_screen, Screen::SearchFields(_)));
        process_bp_events(&mut app).await;
        assert!(!wait_until_search_complete(&app)); // We shouldn't get to the SearchComplete page, so assert that we never get there
        assert!(matches!(app.current_screen, Screen::SearchFields(_)));
        Ok(())
    }
);

#[tokio::test]
#[serial]
async fn test_advanced_regex_negative_lookahead() {
    let temp_dir = &create_test_files!(
        "file1.txt" => text!(
            "This is a test file",
            "It contains some test content",
            "For testing purposes",
        ),
        "file2.txt" => text!(
            "Another test file",
            "With different content",
            "Also for testing",
        ),
        "file3.txt" => text!(
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "something",
        )
    );

    let search_field_values = SearchFieldValues {
        search: FieldValue::new("(test)(?!ing)", false),
        replace: FieldValue::new("BAR", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("", false),
    };

    search_and_replace_test(
        temp_dir,
        &search_field_values,
        &AppRunConfig {
            advanced_regex: true,
            ..AppRunConfig::default()
        },
        vec![
            (Path::new("file1.txt"), 2),
            (Path::new("file2.txt"), 1),
            (Path::new("file3.txt"), 0),
        ],
    )
    .await;

    assert_test_files!(
        temp_dir,
        "file1.txt" => text!(
            "This is a BAR file",
            "It contains some BAR content",
            "For testing purposes",
        ),
        "file2.txt" => text!(
            "Another BAR file",
            "With different content",
            "Also for testing",
        ),
        "file3.txt" => text!(
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "something",
        )
    );
}

test_with_both_regex_modes!(
    test_update_search_results_include_dir,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "dir2/file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something testing",
            ),
            "dir3/file4.txt" => text!(
                "some testing text from dir3/file4.txt, blah",
            ),
            "dir3/subdir1/file5.txt" => text!(
                "some testing text from dir3/subdir1/file5.txt, blah",
            ),
            "dir4/subdir2/file6.txt" => text!(
                "some testing text from dir4/subdir2/file6.txt, blah",
            ),
            "dir4/subdir3/file7.txt" => text!(
                "some testing text from dir4/subdir3/file7.txt, blah",
            ),
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("f", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("dir2/*, dir3/**, */subdir3/*", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("dir1").join("file1.txt"), 0),
                (&Path::new("dir2").join("file2.txt"), 1),
                (&Path::new("dir2").join("file3.txt"), 1),
                (&Path::new("dir3").join("file4.txt"), 1),
                (&Path::new("dir3").join("subdir1").join("file5.txt"), 1),
                (&Path::new("dir4").join("subdir2").join("file6.txt"), 0),
                (&Path::new("dir4").join("subdir3").join("file7.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for f",
            ),
            "dir2/file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something f",
            ),
            "dir3/file4.txt" => text!(
                "some f text from dir3/file4.txt, blah",
            ),
            "dir3/subdir1/file5.txt" => text!(
                "some f text from dir3/subdir1/file5.txt, blah",
            ),
            "dir4/subdir2/file6.txt" => text!(
                "some testing text from dir4/subdir2/file6.txt, blah",
            ),
            "dir4/subdir3/file7.txt" => text!(
                "some f text from dir4/subdir3/file7.txt, blah",
            ),
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_exclude_dir,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "testing""#,
                "}",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("REPL", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("dir1", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("dir1").join("file1.txt"), 0),
                (&Path::new("dir1").join("file1.rs"), 0),
                (&Path::new("dir2").join("file1.txt"), 1),
                (&Path::new("dir2").join("file2.rs"), 1),
                (&Path::new("dir2").join("file3.rs"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "REPL""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "REPL""#,
                "}",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_multiple_includes_and_excludes,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing1() {",
                r#"  "testing1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "testing""#,
                "}",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("REPL", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("dir1/*, *.rs", false),
            exclude_files: FieldValue::new("**/file2.rs", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new("dir1").join("file1.rs"), 2),
                (&Path::new("dir1").join("file2.rs"), 0),
                (&Path::new("dir2").join("file1.txt"), 0),
                (&Path::new("dir2").join("file2.rs"), 0),
                (&Path::new("dir2").join("file3.rs"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            ),
            "dir1/file1.rs" => text!(
                "func REPL1() {",
                r#"  "REPL1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "REPL""#,
                "}",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_multiple_includes_and_excludes_additional_spacing,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing1() {",
                r#"  "testing1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir1/subdir1/subdir2/file3.rs" => text!(
                "func testing3() {",
                r#"  "testing3""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file4.py" => text!(
                "def main():",
                "  return 'testing'",
            ),
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("REPL", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new(" dir1/*,*.rs   ,  *.py", false),
            exclude_files: FieldValue::new("  **/file2.rs ", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new("dir1").join("file1.rs"), 2),
                (&Path::new("dir1").join("file2.rs"), 0),
                (
                    &Path::new("dir1")
                        .join("subdir1")
                        .join("subdir2")
                        .join("file3.rs"),
                    2,
                ),
                (&Path::new("dir2").join("file1.txt"), 0),
                (&Path::new("dir2").join("file2.rs"), 0),
                (&Path::new("dir2").join("file3.rs"), 1),
                (&Path::new("dir2").join("file4.py"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            ),
            "dir1/file1.rs" => text!(
                "func REPL1() {",
                r#"  "REPL1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir1/subdir1/subdir2/file3.rs" => text!(
                "func REPL3() {",
                r#"  "REPL3""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "REPL""#,
                "}",
            ),
            "dir2/file4.py" => text!(
                "def main():",
                "  return 'REPL'",
            ),
        );
        Ok(())
    }
);

test_with_both_regex_modes!(test_ignores_gif_file, |advanced_regex: bool| async move {
    let temp_dir = &create_test_files!(
        "dir1/file1.txt" => text!(
            "This is a text file",
        ),
        "dir2/file2.gif" => text!(
            "This is a gif file",
        ),
        "file3.txt" => text!(
            "This is a text file",
        )
    );

    let search_field_values = SearchFieldValues {
        search: FieldValue::new("is", false),
        replace: FieldValue::new("", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("", false),
    };

    search_and_replace_test(
        temp_dir,
        &search_field_values,
        &AppRunConfig {
            advanced_regex,
            ..AppRunConfig::default()
        },
        vec![
            (&Path::new("dir1").join("file1.txt"), 1),
            (&Path::new("dir2").join("file2.gif"), 0),
            (Path::new("file3.txt"), 1),
        ],
    )
    .await;

    assert_test_files!(
        temp_dir,
        "dir1/file1.txt" => text!(
            "Th  a text file",
        ),
        "dir2/file2.gif" => text!(
            "This is a gif file",
        ),
        "file3.txt" => text!(
            "Th  a text file",
        )
    );
    Ok(())
});

test_with_both_regex_modes!(
    test_ignores_hidden_files_by_default,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This is a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This is a hidden text file",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new(r"\bis\b", false),
            replace: FieldValue::new("REPLACED", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new(".dir2").join("file2.rs"), 0),
                (Path::new(".file3.txt"), 0),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This REPLACED a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This is a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This is a hidden text file",
            )
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_includes_hidden_files_with_flag,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This is a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This is a hidden text file",
            )
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new(r"\bis\b", false),
            replace: FieldValue::new("REPLACED", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                include_hidden: true,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new(".dir2").join("file2.rs"), 1),
                (Path::new(".file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This REPLACED a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This REPLACED a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This REPLACED a hidden text file",
            )
        );
        Ok(())
    }
);

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn read_file<P>(p: P) -> String
where
    P: AsRef<Path>,
{
    fs::read_to_string(p).unwrap().replace("\r\n", "\n")
}

test_with_both_regex_modes!(
    test_binary_file_filtering,
    |advanced_regex: bool| async move {
        let temp_dir = TempDir::new().unwrap();
        let fixtures_dir = "tests/fixtures/binary_test";
        copy_dir_all(format!("{fixtures_dir}/initial"), temp_dir.path())?;

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("sample", false),
            replace: FieldValue::new("REPLACED", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        };

        search_and_replace_test(
            &temp_dir,
            &search_field_values,
            &AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            vec![
                (&Path::new("textfiles").join("code.rs"), 1),
                (&Path::new("textfiles").join("config.json"), 1),
                (&Path::new("textfiles").join("document.txt"), 2),
                (&Path::new("textfiles").join("noextension"), 1),
                (&Path::new("binaries").join("image.wrongext"), 0),
                (&Path::new("binaries").join("document.pdf"), 0),
                (&Path::new("binaries").join("document_pdf_wrong_ext.rs"), 0),
                (&Path::new("binaries").join("archive.zip"), 0),
                (&Path::new("binaries").join("rust_binary"), 0),
            ],
        )
        .await;

        let text_files = vec![
            "textfiles/code.rs",
            "textfiles/config.json",
            "textfiles/document.txt",
            "textfiles/noextension",
        ];

        let binary_files = vec![
            "binaries/image.wrongext",
            "binaries/document.pdf",
            "binaries/document_pdf_wrong_ext.rs",
            "binaries/archive.zip",
            "binaries/rust_binary",
        ];

        for file in &text_files {
            let actual = read_file(temp_dir.path().join(file));
            let expected = read_file(format!("{fixtures_dir}/updated/{file}"));

            assert_eq!(
                actual, expected,
                "Text file {file} was not correctly updated",
            );
        }

        for file in &binary_files {
            let actual = fs::read(temp_dir.path().join(file))?;
            let original = fs::read(format!("{fixtures_dir}/initial/{file}"))?;

            assert_eq!(
                actual, original,
                "Binary file {file} was unexpectedly modified",
            );
        }

        Ok(())
    }
);

#[tokio::test]
async fn test_keymaps_search_fields() {
    let (app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );

    assert!(matches!(app.current_screen, Screen::SearchFields(_)));

    assert_debug_snapshot!("search_fields_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_fields_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_complete() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );

    let cancelled = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::unbounded_channel();
    let mut search_state = SearchState::new(sender, receiver, cancelled);
    search_state.set_search_completed_now();
    app.current_screen = Screen::SearchFields(SearchFieldsState {
        search_state: Some(search_state),
        focussed_section: FocussedSection::SearchResults,
        search_debounce_timer: None,
        preview_update_state: None,
    });

    assert_debug_snapshot!("search_complete_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_complete_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_progressing() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );

    let cancelled = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::unbounded_channel();
    let search_state = SearchState::new(sender, receiver, cancelled);
    app.current_screen = Screen::SearchFields(SearchFieldsState {
        search_state: Some(search_state),
        focussed_section: FocussedSection::SearchResults,
        search_debounce_timer: None,
        preview_update_state: None,
    });

    assert_debug_snapshot!("search_progressing_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_progressing_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_performing_replacement() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );

    let (_sender, receiver) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    app.current_screen = Screen::PerformingReplacement(PerformingReplacementState::new(
        receiver,
        cancelled,
        Arc::new(AtomicUsize::new(0)),
        0,
    ));

    assert_debug_snapshot!(
        "performing_replacement_compact_keymaps",
        app.keymaps_compact()
    );
    assert_debug_snapshot!("performing_replacement_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_results() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );

    let replace_state_with_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![SearchResultWithReplacement {
            search_result: SearchResult {
                path: PathBuf::from("error.txt"),
                line_number: 1,
                line: "test line".to_string(),
                line_ending: LineEnding::Lf,
                included: true,
            },
            replacement: "replacement".to_string(),
            replace_result: Some(ReplaceResult::Error("Test error".to_string())),
        }],
        replacement_errors_pos: 0,
    };
    app.current_screen = Screen::Results(replace_state_with_errors);

    assert_debug_snapshot!("results_with_errors_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("results_with_errors_all_keymaps", app.keymaps_all());

    let replace_state_without_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    };
    app.current_screen = Screen::Results(replace_state_without_errors);

    assert_debug_snapshot!(
        "results_without_errors_compact_keymaps",
        app.keymaps_compact()
    );
    assert_debug_snapshot!("results_without_errors_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_popup() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
    );
    app.add_error(AppError {
        name: "Test".to_string(),
        long: "Test error".to_string(),
    });

    assert_debug_snapshot!("popup_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("popup_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_unlock_prepopulated_fields_via_alt_u() {
    let search_field_values = SearchFieldValues {
        search: FieldValue::new("test_search", true),
        replace: FieldValue::new("", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("*.rs", true),
        exclude_files: FieldValue::new("", false),
    };

    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &search_field_values,
        &AppRunConfig::default(),
    );

    assert_eq!(
        app.search_fields
            .fields
            .iter()
            .map(|f| f.set_by_cli)
            .collect::<Vec<_>>(),
        vec![true, false, false, false, false, true, false]
    );

    let key_event = KeyEvent {
        code: KeyCode::Char('u'),
        modifiers: KeyModifiers::ALT,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    };

    let result = app.handle_key_event(&key_event);
    assert_eq!(result, EventHandlingResult::Rerender);

    for field in &app.search_fields.fields {
        assert!(!field.set_by_cli);
    }
}

#[tokio::test]
async fn test_keybinding_integration_with_disabled_fields() {
    let search_field_values = SearchFieldValues {
        search: FieldValue::new("function", true),
        replace: FieldValue::new("method", true),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("*.rs", true),
        exclude_files: FieldValue::new("", false),
    };

    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &search_field_values,
        &AppRunConfig::default(),
    );

    assert_eq!(app.search_fields.highlighted, 2);

    let tab_event = KeyEvent {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::NONE,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    };

    app.handle_key_event(&tab_event);
    assert_eq!(app.search_fields.highlighted, 3);

    app.handle_key_event(&tab_event);
    assert_eq!(app.search_fields.highlighted, 4);

    app.handle_key_event(&tab_event);
    assert_eq!(app.search_fields.highlighted, 6);
    let backtab_event = KeyEvent {
        code: KeyCode::BackTab,
        modifiers: KeyModifiers::NONE,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    };

    app.handle_key_event(&backtab_event);
    assert_eq!(app.search_fields.highlighted, 4);
}

#[tokio::test]
async fn test_alt_u_unlocks_all_fields() {
    let search_field_values = SearchFieldValues {
        search: FieldValue::new("search", true),
        replace: FieldValue::new("replace", true),
        fixed_strings: FieldValue::new(true, true),
        match_whole_word: FieldValue::new(false, true),
        match_case: FieldValue::new(true, true),
        include_files: FieldValue::new("*.rs", true),
        exclude_files: FieldValue::new("*.txt", true),
    };

    let (mut app, _app_event_receiver) = App::new_with_receiver(
        current_dir().unwrap(),
        &search_field_values,
        &AppRunConfig::default(),
    );

    for field in &app.search_fields.fields {
        assert!(field.set_by_cli);
    }

    let x_char_event = KeyEvent {
        code: KeyCode::Char('x'),
        modifiers: KeyModifiers::NONE,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    };

    app.handle_key_event(&x_char_event);
    assert_eq!(app.search_fields.search().text(), "search"); // Shouldn't have changed - all fields locked

    let alt_u_event = KeyEvent {
        code: KeyCode::Char('u'),
        modifiers: KeyModifiers::ALT,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    };

    app.handle_key_event(&alt_u_event);

    for field in &app.search_fields.fields {
        assert!(!field.set_by_cli);
    }

    app.handle_key_event(&x_char_event);
    assert_eq!(app.search_fields.search().text(), "searchx");
}
