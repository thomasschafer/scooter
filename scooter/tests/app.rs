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
use std::env::current_dir;
use std::mem;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::mpsc;

use scooter::{
    app::{App, AppError, AppRunConfig, SearchState},
    fields::{FieldValue, SearchFieldValues, SearchFields},
    replace::{PerformingReplacementState, ReplaceState},
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

// TODO(autosearch): delete this commented out code

// pub fn wait_until<F>(condition: F, timeout: Duration) -> bool
// where
//     F: Fn() -> bool,
// {
//     let start = Instant::now();
//     let sleep_duration = max(timeout / 50, Duration::from_millis(1));
//     while !condition() && start.elapsed() <= timeout {
//         sleep(sleep_duration);
//     }
//     condition()
// }

// async fn process_bp_events(app: &mut App) {
//     let timeout = Duration::from_secs(5);

//     // TODO: will this channel ever close? Should we change testing strategy?
//     let timeout_res = tokio::time::timeout(timeout, async {
//         while let Some(event) = app.background_processing_recv().await {
//             app.handle_background_processing_event(event);
//         }
//     })
//     .await;
//     assert!(
//         timeout_res.is_ok(),
//         "Couldn't process background events in a reasonable time"
//     );
// }

// macro_rules! wait_for_screen {
//     ($app:expr, $variant:path) => {
//         wait_until(
//             || matches!($app.current_screen, $variant(_)),
//             Duration::from_secs(1),
//         )
//     };
// }

// fn setup_app(
//     temp_dir: &TempDir,
//     search_field_values: &SearchFieldValues<'_>,
//     app_run_config: &AppRunConfig,
// ) -> App {
//     let (app, _app_event_receiver) = App::new_with_receiver(
//         temp_dir.path().to_path_buf(),
//         search_field_values,
//         app_run_config,
//     );
//     app
// }

// // TODO(autosave): move these to app_runner tests
// // TODO: simplify this test - it is somewhat tied to the current implementation
// async fn search_and_replace_test(
//     temp_dir: &TempDir,
//     search_field_values: &SearchFieldValues<'_>,
//     app_run_config: &AppRunConfig,
//     expected_matches: Vec<(&Path, usize)>,
// ) {
//     let total_num_expected_matches = expected_matches
//         .iter()
//         .map(|(_, count)| count)
//         .sum::<usize>();

//     let mut app = setup_app(temp_dir, search_field_values, app_run_config);
//     assert_eq!(app.errors(), vec![]);
//     let res = app.perform_search_if_valid();
//     assert!(res != EventHandlingResult::Exit(None));

//     process_bp_events(&mut app).await;
//     assert!(wait_until_search_complete(&app));

//     let Screen::SearchFields(SearchFieldsState {
//         search_state: Some(state),
//         ..
//     }) = &mut app.current_screen
//     else {
//         panic!(
//             "Expected SearchComplete results with Some search state, found {:?}",
//             app.current_screen
//         );
//     };

//     for (file_path, num_expected_matches) in &expected_matches {
//         let num_actual_matches = state
//             .results
//             .iter()
//             .filter(|result| {
//                 let result_path = result.search_result.path.to_str().unwrap();
//                 let file_path = file_path.to_str().unwrap();
//                 result_path == temp_dir.path().join(file_path).to_string_lossy()
//             })
//             .count();
//         let num_expected_matches = *num_expected_matches;
//         assert_eq!(
//             num_actual_matches,
//             num_expected_matches,
//             "{}: expected {num_expected_matches}, found {num_actual_matches}",
//             file_path.display(),
//         );
//     }

//     assert_eq!(state.results.len(), total_num_expected_matches);

//     app.trigger_replacement();

//     process_bp_events(&mut app).await;
//     assert!(wait_for_screen!(&app, Screen::Results));

//     if let Screen::Results(search_state) = &app.current_screen {
//         assert_eq!(search_state.num_successes, total_num_expected_matches);
//         assert_eq!(search_state.num_ignored, 0);
//         assert_eq!(search_state.errors.len(), 0);
//     } else {
//         panic!(
//             "Expected screen to be Screen::Results, instead found {:?}",
//             app.current_screen
//         );
//     }
// }

// fn wait_until_search_complete(app: &App) -> bool {
//     wait_until(
//         || {
//             if let Screen::SearchFields(ref state) = app.current_screen {
//                 state
//                     .search_state
//                     .as_ref()
//                     .is_some_and(SearchState::search_has_completed)
//             } else {
//                 false
//             }
//         },
//         Duration::from_secs(1),
//     )
// }

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
