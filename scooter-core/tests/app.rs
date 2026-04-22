use insta::assert_debug_snapshot;
use scooter_core::{
    app::{
        AppEvent, BackgroundProcessingEvent, Event, EventHandlingResult, InputSource, InternalEvent,
    },
    errors::AppError,
    fields::{FieldValue, SearchFieldValues, SearchFields},
    keyboard::KeyEvent,
    replace::{PerformingReplacementState, ReplaceState},
};
use scooter_core::{
    line_reader::LineEnding,
    replace::ReplaceResult,
    search::{SearchResult, SearchResultWithReplacement},
};
use std::{
    env::current_dir,
    mem,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, atomic::AtomicUsize, atomic::Ordering},
    time::Duration,
};
use tokio::sync::mpsc;

use scooter_core::{
    app::{
        App, AppRunConfig, FocussedSection, Popup, Screen, SearchFieldsState, SearchPhase,
        SearchState,
    },
    config::Config,
    keyboard::{KeyCode as ScooterKeyCode, KeyModifiers as ScooterKeyModifiers},
};

const EVENT_TIMEOUT: Duration = Duration::from_millis(2_000);
const PRESERVED_DEBOUNCE_MAX_WAIT: Duration = Duration::from_millis(200);

#[tokio::test]
async fn test_replace_state() {
    let mut state = ReplaceState {
        num_successes: 2,
        num_ignored: 1,
        errors: (1..3)
            .map(|n| SearchResultWithReplacement {
                search_result: SearchResult::new_line(
                    Some(PathBuf::from(format!("error-{n}.txt"))),
                    1,
                    format!("line {n}"),
                    LineEnding::Lf,
                    true,
                ),
                replacement: format!("error replacement {n}"),
                replace_result: Some(ReplaceResult::Error(format!("Test error {n}"))),
                preview_error: None,
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
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();
    app.ui_state.current_screen = Screen::Results(ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    });

    app.reset();

    assert!(matches!(
        app.ui_state.current_screen,
        Screen::SearchFields(_)
    ));
}

#[tokio::test]
async fn test_back_from_results() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();
    let (sender, receiver) = mpsc::unbounded_channel();
    let mut state = SearchFieldsState::default();
    state.focussed_section = FocussedSection::SearchResults;
    state.search_state = Some(SearchState::new(
        sender,
        receiver,
        Arc::new(AtomicBool::new(false)),
    ));
    app.ui_state.current_screen = Screen::SearchFields(state);
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

    let res = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('o'),
        ScooterKeyModifiers::CONTROL,
    ));
    assert!(!matches!(res, EventHandlingResult::Exit(None)));
    assert_eq!(app.search_fields.search().text(), "foo");
    assert_eq!(app.search_fields.replace().text(), "bar");
    assert!(app.search_fields.fixed_strings().checked);
    assert_eq!(app.search_fields.include_files().text(), "pattern");
    assert_eq!(app.search_fields.exclude_files().text(), "");
    assert!(matches!(
        app.ui_state.current_screen,
        Screen::SearchFields(_)
    ));
}

fn test_error_popup_invalid_input_impl(search_fields: &SearchFieldValues<'_>) {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        search_fields,
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    // Simulate search being triggered in background
    app.perform_search_background();
    assert!(app.popup().is_none());

    // Hitting enter should show popup
    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Enter,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(
        app.ui_state.current_screen,
        Screen::SearchFields(_)
    ));
    assert!(matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Esc,
        ScooterKeyModifiers::NONE,
    ));
    assert!(!matches!(res, EventHandlingResult::Exit(None)));
    assert!(!matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('c'),
        ScooterKeyModifiers::CONTROL,
    ));
    assert!(matches!(res, EventHandlingResult::Exit(None)));
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
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();
    let screen_variant = std::mem::discriminant(&initial_screen);
    app.ui_state.current_screen = initial_screen;

    assert!(app.popup().is_none());
    assert_eq!(
        mem::discriminant(&app.ui_state.current_screen),
        screen_variant
    );

    let res_open = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('h'),
        ScooterKeyModifiers::CONTROL,
    ));
    assert!(matches!(res_open, EventHandlingResult::Rerender));
    assert!(matches!(app.popup(), Some(Popup::Help)));
    assert_eq!(
        std::mem::discriminant(&app.ui_state.current_screen),
        screen_variant
    );

    let res_close = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Esc,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(res_close, EventHandlingResult::Rerender));
    assert!(app.popup().is_none());
    assert_eq!(
        std::mem::discriminant(&app.ui_state.current_screen),
        screen_variant
    );
}

#[tokio::test]
async fn test_help_popup_on_search_fields() {
    test_help_popup_on_screen(Screen::SearchFields(SearchFieldsState::default()));
}

#[tokio::test]
async fn test_help_popup_on_search_results() {
    let (sender, receiver) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut state = SearchFieldsState::default();
    state.focussed_section = FocussedSection::SearchResults;
    state.search_state = Some(SearchState::new(sender, receiver, cancelled));
    let initial_screen = Screen::SearchFields(state);
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

#[tokio::test]
async fn test_keymaps_search_fields() {
    let app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert!(matches!(
        app.ui_state.current_screen,
        Screen::SearchFields(_)
    ));

    assert_debug_snapshot!("search_fields_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_fields_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_complete() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let cancelled = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::unbounded_channel();
    let mut search_state = SearchState::new(sender, receiver, cancelled);
    search_state.set_complete_now();
    let mut state = SearchFieldsState::default();
    state.search_state = Some(search_state);
    state.focussed_section = FocussedSection::SearchResults;
    app.ui_state.current_screen = Screen::SearchFields(state);

    assert_debug_snapshot!("search_complete_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_complete_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_progressing() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let cancelled = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::unbounded_channel();
    let search_state = SearchState::new(sender, receiver, cancelled);
    let mut state = SearchFieldsState::default();
    state.search_state = Some(search_state);
    state.focussed_section = FocussedSection::SearchResults;
    app.ui_state.current_screen = Screen::SearchFields(state);

    assert_debug_snapshot!("search_progressing_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_progressing_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_performing_replacement() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let (_sender, receiver) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    app.ui_state.current_screen = Screen::PerformingReplacement(PerformingReplacementState::new(
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
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let replace_state_with_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![SearchResultWithReplacement {
            search_result: SearchResult::new_line(
                Some(PathBuf::from("error.txt")),
                1,
                "test line".to_string(),
                LineEnding::Lf,
                true,
            ),
            replacement: "replacement".to_string(),
            replace_result: Some(ReplaceResult::Error("Test error".to_string())),
            preview_error: None,
        }],
        replacement_errors_pos: 0,
    };
    app.ui_state.current_screen = Screen::Results(replace_state_with_errors);

    assert_debug_snapshot!("results_with_errors_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("results_with_errors_all_keymaps", app.keymaps_all());

    let replace_state_without_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    };
    app.ui_state.current_screen = Screen::Results(replace_state_without_errors);

    assert_debug_snapshot!(
        "results_without_errors_compact_keymaps",
        app.keymaps_compact()
    );
    assert_debug_snapshot!("results_without_errors_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_popup() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();
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

    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &search_field_values,
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert_eq!(
        app.search_fields
            .fields
            .iter()
            .map(|f| f.set_by_cli)
            .collect::<Vec<_>>(),
        vec![true, false, false, false, false, true, false]
    );

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('u'),
        ScooterKeyModifiers::ALT,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));

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

    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &search_field_values,
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert_eq!(app.search_fields.highlighted, 2);

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Tab,
        ScooterKeyModifiers::NONE,
    ));
    assert_eq!(app.search_fields.highlighted, 3);

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Tab,
        ScooterKeyModifiers::NONE,
    ));
    assert_eq!(app.search_fields.highlighted, 4);

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Tab,
        ScooterKeyModifiers::NONE,
    ));
    assert_eq!(app.search_fields.highlighted, 6);

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Tab,
        ScooterKeyModifiers::SHIFT,
    ));
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

    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &search_field_values,
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    for field in &app.search_fields.fields {
        assert!(field.set_by_cli);
    }

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('x'),
        ScooterKeyModifiers::NONE,
    ));
    assert_eq!(app.search_fields.search().text(), "search"); // Shouldn't have changed - all fields locked

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('u'),
        ScooterKeyModifiers::ALT,
    ));

    for field in &app.search_fields.fields {
        assert!(!field.set_by_cli);
    }

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('x'),
        ScooterKeyModifiers::NONE,
    ));
    assert_eq!(app.search_fields.search().text(), "searchx");
}

#[tokio::test]
async fn test_handle_key_event_quit_with_ctrl_c_takes_precedence_over_popup() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    app.add_error(AppError {
        name: "Test error".to_string(),
        long: "Test error details".to_string(),
    });
    assert!(app.popup().is_some());

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('c'),
        ScooterKeyModifiers::CONTROL,
    ));

    assert!(matches!(result, EventHandlingResult::Exit(None)));
}

#[tokio::test]
async fn test_handle_key_event_unmapped_key_closes_popup() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    app.add_error(AppError {
        name: "Test error".to_string(),
        long: "Test error details".to_string(),
    });
    assert!(app.popup().is_some());

    // Press an unmapped key (not a command)
    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('z'),
        ScooterKeyModifiers::NONE,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert!(app.popup().is_none());
}

#[tokio::test]
async fn test_handle_key_event_unmapped_key_in_search_fields_focus_enters_chars() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            search: FieldValue::new("test", false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let Screen::SearchFields(ref state) = app.ui_state.current_screen else {
        panic!(
            "Expected Screen::SearchFields, found {:?}",
            app.ui_state.current_screen
        );
    };
    assert_eq!(state.focussed_section, FocussedSection::SearchFields);

    let initial_text = app.search_fields.search().text().to_string();

    // Press a character that's not mapped to a command
    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('x'),
        ScooterKeyModifiers::NONE,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert_eq!(
        app.search_fields.search().text(),
        format!("{initial_text}x")
    );
}

#[tokio::test]
async fn test_handle_key_event_reset_command() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            search: FieldValue::new("test", false),
            replace: FieldValue::new("replacement", false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert_eq!(app.search_fields.search().text(), "test");

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('r'),
        ScooterKeyModifiers::CONTROL,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert_eq!(app.search_fields.search().text(), "");
    assert_eq!(app.search_fields.replace().text(), "");
}

#[tokio::test]
async fn test_handle_key_event_toggle_preview_wrapping() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let initial_wrap = app.config.preview.wrap_text;

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('l'),
        ScooterKeyModifiers::CONTROL,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert_eq!(app.config.preview.wrap_text, !initial_wrap);
}

#[tokio::test]
async fn test_toggle_escape_sequences_updates_preview_without_restarting_search() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            search: FieldValue::new("foo", false),
            replace: FieldValue::new(r"X\nY", false),
            fixed_strings: FieldValue::new(true, false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let cancelled = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::unbounded_channel();
    let mut search_state = SearchState::new(sender, receiver, cancelled);
    search_state.results.push(SearchResultWithReplacement {
        search_result: SearchResult::new_line(
            Some(PathBuf::from("file.txt")),
            1,
            "foo".to_string(),
            LineEnding::Lf,
            true,
        ),
        replacement: "stale replacement".to_string(),
        replace_result: None,
        preview_error: None,
    });
    search_state.set_complete_now();
    let mut state = SearchFieldsState::default();
    state.focussed_section = FocussedSection::SearchResults;
    state.search_state = Some(search_state);
    app.ui_state.current_screen = Screen::SearchFields(state);

    assert!(app.search_has_completed());

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('e'),
        ScooterKeyModifiers::ALT,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert!(app.run_config.interpret_escape_sequences);
    assert!(app.search_has_completed());

    let Screen::SearchFields(state) = &app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    let search_state = state
        .search_state
        .as_ref()
        .expect("Expected existing search state");
    assert_eq!(search_state.results.len(), 1);
    assert_eq!(search_state.results[0].replacement, "X\nY");
    assert!(state.preview_update_state.is_some());
}

#[tokio::test]
async fn test_toggle_escape_sequences_without_search_state_only_toggles_flag() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            search: FieldValue::new("foo", false),
            replace: FieldValue::new(r"X\nY", false),
            fixed_strings: FieldValue::new(true, false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();
    app.ui_state.current_screen = Screen::SearchFields(SearchFieldsState::default());

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('e'),
        ScooterKeyModifiers::ALT,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert!(app.run_config.interpret_escape_sequences);
    let Screen::SearchFields(state) = &app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    assert!(state.search_state.is_none());
    assert!(state.preview_update_state.is_none());
}

#[tokio::test]
async fn test_toggle_escape_sequences_keeps_pending_debounced_search() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            replace: FieldValue::new(r"X\nY", false),
            fixed_strings: FieldValue::new(true, false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    // Typing in the search field should queue a debounced PerformSearch event.
    let type_search = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('f'),
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(type_search, EventHandlingResult::Rerender));

    let Screen::SearchFields(state) = &app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    assert!(state.search_debounce_timer.is_some());

    // Toggle escape interpretation before the debounce fires.
    let toggle = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('e'),
        ScooterKeyModifiers::ALT,
    ));
    assert!(matches!(toggle, EventHandlingResult::Rerender));
    assert!(app.run_config.interpret_escape_sequences);
    assert_eq!(
        app.searcher.as_ref().expect("Expected searcher").replace(),
        "X\nY"
    );

    let Screen::SearchFields(state) = &app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    assert!(state.search_debounce_timer.is_some());

    // Wait long enough for the queued debounce timer to emit its app event.
    tokio::time::sleep(Duration::from_millis(330)).await;

    let queued = tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
        .await
        .expect("Expected queued debounce event");
    let generation = queued_search_generation(queued);

    // Handling the queued event should keep replacement config aligned with the toggle.
    let handled =
        app.handle_internal_event(InternalEvent::App(AppEvent::PerformSearch { generation }));
    assert!(matches!(handled, EventHandlingResult::Rerender));
    assert_eq!(
        app.searcher.as_ref().expect("Expected searcher").replace(),
        "X\nY"
    );
}

#[tokio::test]
async fn test_handle_key_event_show_help_menu() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert!(app.popup().is_none());

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('h'),
        ScooterKeyModifiers::CONTROL,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert!(matches!(app.popup(), Some(Popup::Help)));
}

#[tokio::test]
async fn test_handle_key_event_enter_triggers_search_from_fields() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            search: FieldValue::new("test", false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let Screen::SearchFields(ref state) = app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    assert_eq!(state.focussed_section, FocussedSection::SearchFields);

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Enter,
        ScooterKeyModifiers::NONE,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    let Screen::SearchFields(ref state) = app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    assert_eq!(state.focussed_section, FocussedSection::SearchResults);
}

#[tokio::test]
async fn test_handle_key_event_backspace_in_search_fields() {
    let mut app = App::new(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues {
            search: FieldValue::new("test", false),
            ..Default::default()
        },
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Backspace,
        ScooterKeyModifiers::NONE,
    ));

    assert!(matches!(result, EventHandlingResult::Rerender));
    assert_eq!(app.search_fields.search().text(), "tes");
}

// -----------------------------------------------------------------------------
// Tests for the synchronous search-phase model and empty-search short-circuit.
// See `handle_search_fields_input` in app.rs for the behaviour under test.
// -----------------------------------------------------------------------------

/// Empty stdin haystack — no file walk, no asynchronous background noise.
/// Use for tests that don't care about `DirSearchKey` equality.
fn stdin_source() -> InputSource {
    InputSource::Stdin(Arc::new(String::new()))
}

/// Build an app with search fields pre-populated and a `SearchState`
/// stitched in at the given phase. Callers pick the input source — use
/// `stdin_source()` for an async-quiet default, or a directory source when
/// the test needs to exercise `DirSearchKey`.
fn build_test_app_with_phase(
    input_source: InputSource,
    search_text: &str,
    phase: SearchPhase,
    results: Vec<SearchResultWithReplacement>,
) -> App {
    let mut app = App::new(
        input_source,
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();
    app.search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new(search_text, false),
            ..Default::default()
        },
        true,
    );
    app.searcher = app
        .validate_fields()
        .expect("validation should not error on plain text");

    let (sender, receiver) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut state = SearchState::new(sender, receiver, cancelled);
    state.results = results;
    state.phase = phase;

    let last_scheduled_key = if search_text.is_empty() {
        None
    } else {
        Some(Box::new(app.current_search_key()))
    };
    let mut search_fields_state = SearchFieldsState::default();
    search_fields_state.focussed_section = FocussedSection::SearchFields;
    search_fields_state.search_state = Some(state);
    search_fields_state.last_scheduled_key = last_scheduled_key;
    app.ui_state.current_screen = Screen::SearchFields(search_fields_state);
    app
}

fn search_fields_state(app: &App) -> &SearchFieldsState {
    let Screen::SearchFields(state) = &app.ui_state.current_screen else {
        panic!(
            "Expected SearchFields screen, found {:?}",
            app.ui_state.current_screen
        );
    };
    state
}

fn type_char(app: &mut App, c: char) -> EventHandlingResult {
    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char(c),
        ScooterKeyModifiers::NONE,
    ))
}

fn queued_search_generation(event: Event) -> u64 {
    match event {
        Event::Internal(InternalEvent::App(AppEvent::PerformSearch { generation })) => generation,
        other => panic!("Expected queued PerformSearch event, got {other:?}"),
    }
}

fn dummy_result() -> SearchResultWithReplacement {
    SearchResultWithReplacement {
        search_result: SearchResult::new_line(
            Some(PathBuf::from("a.txt")),
            1,
            "line".to_owned(),
            LineEnding::Lf,
            true,
        ),
        replacement: "line".to_owned(),
        replace_result: None,
        preview_error: None,
    }
}

#[tokio::test]
async fn test_typing_after_complete_search_transitions_phase_to_pending() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "foo",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![dummy_result()],
    );

    let result = type_char(&mut app, 'x');
    assert!(matches!(result, EventHandlingResult::Rerender));

    let state = search_fields_state(&app);
    let phase = state
        .search_state
        .as_ref()
        .expect("search state should still exist")
        .phase;
    assert!(
        matches!(phase, SearchPhase::Pending),
        "typing after Complete should flip phase to Pending, got {phase:?}"
    );
    assert!(state.search_debounce_timer.is_some());
}

#[tokio::test]
async fn test_phase_transitions_pending_running_complete() {
    // Starts empty: typing the first char schedules a debounce but leaves
    // `search_state` as None until `perform_search_already_validated` runs.
    let mut app = App::new(
        InputSource::Stdin(Arc::new(String::new())),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert!(matches!(
        type_char(&mut app, 'a'),
        EventHandlingResult::Rerender
    ));
    assert!(search_fields_state(&app).search_debounce_timer.is_some());

    // Wait for the debounce to emit `PerformSearch`, then handle it.
    tokio::time::sleep(Duration::from_millis(330)).await;
    let queued = tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
        .await
        .expect("debounce should have emitted PerformSearch");
    let generation = queued_search_generation(queued);
    app.handle_internal_event(InternalEvent::App(AppEvent::PerformSearch { generation }));

    let phase_after_perform = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("perform should have created a state")
        .phase;
    assert!(
        matches!(phase_after_perform, SearchPhase::Running { .. }),
        "expected Running after PerformSearch, got {phase_after_perform:?}"
    );

    // Simulate the background task's completion event.
    app.handle_background_processing_event(BackgroundProcessingEvent::SearchCompleted);
    let phase_after_complete = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("state should still exist")
        .phase;
    assert!(
        matches!(phase_after_complete, SearchPhase::Complete { .. }),
        "expected Complete after SearchCompleted, got {phase_after_complete:?}"
    );
}

#[tokio::test]
async fn test_clearing_search_synchronously_clears_state() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "a",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![dummy_result()],
    );

    // Backspace deletes the only char, leaving the search empty.
    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Backspace,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));

    let state = search_fields_state(&app);
    assert!(state.search_state.is_none());
    assert!(state.search_debounce_timer.is_none());
    assert!(state.last_scheduled_key.is_none());
}

#[tokio::test]
async fn test_clearing_search_does_not_schedule_perform_search() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "a",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![],
    );

    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Backspace,
        ScooterKeyModifiers::NONE,
    ));

    // Wait past the debounce window and confirm no PerformSearch event shows up.
    tokio::time::sleep(Duration::from_millis(330)).await;
    let queued = tokio::time::timeout(Duration::from_millis(50), app.event_recv()).await;
    assert!(
        queued.is_err(),
        "no debounce should have fired for an empty search, got {queued:?}"
    );
}

#[tokio::test]
async fn test_retyping_after_clear_runs_a_fresh_search() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "a",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![],
    );

    // Clear then retype.
    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Backspace,
        ScooterKeyModifiers::NONE,
    ));
    assert!(type_char(&mut app, 'b').is_rerender());

    assert!(search_fields_state(&app).search_debounce_timer.is_some());

    tokio::time::sleep(Duration::from_millis(330)).await;
    let queued = tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
        .await
        .expect("retyping after clear should queue a PerformSearch");
    let _generation = queued_search_generation(queued);
}

#[tokio::test]
async fn test_reverting_after_temporary_invalid_search_requeues_debounce() {
    let mut app = App::new(
        stdin_source(),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert!(type_char(&mut app, 'a').is_rerender());
    assert!(type_char(&mut app, 'b').is_rerender());
    assert!(type_char(&mut app, 'c').is_rerender());
    assert!(search_fields_state(&app).search_debounce_timer.is_some());

    assert!(type_char(&mut app, '(').is_rerender());
    let state_after_invalid = search_fields_state(&app);
    assert!(
        state_after_invalid.search_debounce_timer.is_none(),
        "invalid search should abort the pending debounce"
    );
    assert!(
        state_after_invalid.last_scheduled_key.is_none(),
        "invalid search should clear the last scheduled key so reverting can requeue"
    );

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Backspace,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));
    assert!(search_fields_state(&app).search_debounce_timer.is_some());

    tokio::time::sleep(Duration::from_millis(330)).await;
    let queued = tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
        .await
        .expect("reverting to the last valid query should queue a PerformSearch");
    let _generation = queued_search_generation(queued);
}

#[tokio::test]
async fn test_stale_debounce_event_ignored_after_new_valid_edit() {
    let mut app = App::new(
        stdin_source(),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert!(type_char(&mut app, 'a').is_rerender());
    tokio::time::sleep(Duration::from_millis(330)).await;
    let stale_generation = queued_search_generation(
        tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
            .await
            .expect("first debounce should have emitted PerformSearch"),
    );

    assert!(type_char(&mut app, 'b').is_rerender());
    let handled = app.handle_internal_event(InternalEvent::App(AppEvent::PerformSearch {
        generation: stale_generation,
    }));
    assert!(
        matches!(handled, EventHandlingResult::None),
        "stale debounce event should be ignored after a newer valid edit"
    );
    assert!(
        search_fields_state(&app).search_state.is_none(),
        "stale event must not start a search early"
    );

    tokio::time::sleep(Duration::from_millis(330)).await;
    let fresh_generation = queued_search_generation(
        tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
            .await
            .expect("second debounce should have emitted PerformSearch"),
    );
    assert_ne!(stale_generation, fresh_generation);
    let handled = app.handle_internal_event(InternalEvent::App(AppEvent::PerformSearch {
        generation: fresh_generation,
    }));
    assert!(matches!(handled, EventHandlingResult::Rerender));
    let phase = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("fresh event should start a search")
        .phase;
    assert!(
        matches!(phase, SearchPhase::Running { .. }),
        "fresh debounce event should start the current search, got {phase:?}"
    );
}

#[tokio::test]
async fn test_stale_debounce_event_ignored_when_current_query_invalid() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "foo",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![dummy_result()],
    );

    assert!(type_char(&mut app, 'x').is_rerender());
    tokio::time::sleep(Duration::from_millis(330)).await;
    let stale_generation = queued_search_generation(
        tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
            .await
            .expect("debounce should have emitted PerformSearch"),
    );

    assert!(type_char(&mut app, '(').is_rerender());
    let phase_after_invalid = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("stale results should remain visible")
        .phase;
    assert!(
        matches!(phase_after_invalid, SearchPhase::Invalid),
        "invalid edit should transition to Invalid, got {phase_after_invalid:?}"
    );

    let handled = app.handle_internal_event(InternalEvent::App(AppEvent::PerformSearch {
        generation: stale_generation,
    }));
    assert!(
        matches!(handled, EventHandlingResult::None),
        "stale debounce event should be ignored once the current query is invalid"
    );
    let phase_after_drop = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("stale results should still be visible")
        .phase;
    assert!(matches!(phase_after_drop, SearchPhase::Invalid));
}

#[tokio::test]
async fn test_stale_results_preserved_while_pending() {
    let started = std::time::Instant::now();
    let results = vec![dummy_result(), dummy_result(), dummy_result()];
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "foo",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        results,
    );

    type_char(&mut app, 'd');

    let state = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("state should persist across an edit");
    // Intentional: keep stale results visible to avoid flicker.
    assert_eq!(state.results.len(), 3);
    assert!(matches!(state.phase, SearchPhase::Pending));
}

/// A superseded search task may still emit one last batch of results after
/// its cancelled flag is set (the flag is polled between iterations, not
/// between channel sends). Those batches must not be appended to the
/// stale-but-displayed result list — otherwise the count visibly
/// increments against an old query, and replacements get computed against
/// the *new* searcher over old match positions.
#[tokio::test]
async fn test_cancelled_state_drops_incoming_search_results() {
    let started = std::time::Instant::now();
    let initial = vec![dummy_result(), dummy_result()];
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "foo",
        SearchPhase::Running { started },
        initial,
    );
    // Simulate the user editing the search: cancel the in-flight task and
    // flip to Pending (matching what `enter_chars_into_field` does).
    let state = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("state fixture should have search state");
    state.cancel();

    // A batch from the now-superseded task arrives. Line content matches
    // the searcher for "foo" so it would be appended if not for the
    // cancellation check.
    app.handle_background_processing_event(BackgroundProcessingEvent::AddSearchResult(
        SearchResult::new_line(
            Some(PathBuf::from("late.txt")),
            1,
            "late foo".to_owned(),
            LineEnding::Lf,
            true,
        ),
    ));

    let state = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("state should still exist");
    assert_eq!(
        state.results.len(),
        2,
        "late batch from the cancelled search must not be appended"
    );
}

#[tokio::test]
async fn test_invalid_edit_cancels_running_search_and_drops_late_results() {
    let started = std::time::Instant::now();
    let initial = vec![dummy_result(), dummy_result()];
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "foo",
        SearchPhase::Running { started },
        initial,
    );

    assert!(type_char(&mut app, '(').is_rerender());
    let state_after_invalid = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("stale results should remain visible");
    assert!(
        state_after_invalid.cancelled.load(Ordering::Relaxed),
        "invalid edit should cancel the in-flight search"
    );
    assert!(
        matches!(state_after_invalid.phase, SearchPhase::Invalid),
        "invalid edit should transition to Invalid, got {:?}",
        state_after_invalid.phase
    );

    app.handle_background_processing_event(BackgroundProcessingEvent::AddSearchResult(
        SearchResult::new_line(
            Some(PathBuf::from("late.txt")),
            1,
            "late foo".to_owned(),
            LineEnding::Lf,
            true,
        ),
    ));

    let state_after_late_batch = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("state should still exist");
    assert_eq!(
        state_after_late_batch.results.len(),
        2,
        "late batch from the invalidated search must not be appended"
    );
}

/// `cancel_in_progress_tasks` is the "stop everything async" primitive used
/// by `reset()`. It already cancelled the search + replacement + preview
/// updates, but not the debounce timer until recently. Pin that behaviour
/// down with a direct test.
#[tokio::test]
async fn test_cancel_in_progress_tasks_aborts_search_debounce() {
    let mut app = App::new(
        stdin_source(),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    // Typing schedules a debounce timer on the search fields state.
    assert!(type_char(&mut app, 'a').is_rerender());
    assert!(
        search_fields_state(&app).search_debounce_timer.is_some(),
        "sanity: typing should have scheduled a debounce"
    );

    app.cancel_in_progress_tasks();

    assert!(
        search_fields_state(&app).search_debounce_timer.is_none(),
        "cancel_in_progress_tasks must abort the pending search debounce"
    );
}

#[tokio::test]
async fn test_cursor_movement_skips_redundant_search_debounce() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "abc",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![],
    );

    // Left arrow moves the cursor but leaves every search-relevant input unchanged.
    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Left,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));

    let state = search_fields_state(&app);
    assert!(
        state.search_debounce_timer.is_none(),
        "cursor move should not schedule a new debounce"
    );
    let phase = state
        .search_state
        .as_ref()
        .expect("state should be preserved")
        .phase;
    assert!(
        matches!(phase, SearchPhase::Complete { .. }),
        "phase should stay Complete when no re-search is triggered, got {phase:?}"
    );
}

#[tokio::test]
async fn test_cursor_movement_preserves_pending_search_debounce() {
    let mut app = App::new(
        stdin_source(),
        &SearchFieldValues::default(),
        AppRunConfig::default(),
        Config::default(),
    )
    .unwrap();

    assert!(type_char(&mut app, 'a').is_rerender());
    assert!(
        search_fields_state(&app).search_debounce_timer.is_some(),
        "typing should schedule a debounce"
    );

    tokio::time::sleep(Duration::from_millis(225)).await;

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Left,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));
    assert!(
        search_fields_state(&app).search_debounce_timer.is_some(),
        "cursor move must not cancel the pending debounce"
    );

    let wait_started = std::time::Instant::now();
    let queued = tokio::time::timeout(EVENT_TIMEOUT, app.event_recv())
        .await
        .expect("existing debounce should still emit PerformSearch");
    let wait_elapsed = wait_started.elapsed();
    assert!(
        wait_elapsed < PRESERVED_DEBOUNCE_MAX_WAIT,
        "cursor move appears to have restarted the debounce; waited {wait_elapsed:?}"
    );
    let _generation = queued_search_generation(queued);
}

/// Counterpart to the stdin-source cursor-movement test, exercising
/// `DirSearchKey`'s derived `PartialEq` so a future change to its field set
/// that breaks equality is caught here rather than only in integration tests.
#[tokio::test]
async fn test_cursor_movement_skips_redundant_search_debounce_directory_source() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        InputSource::Directory(current_dir().unwrap()),
        "abc",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![],
    );

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Left,
        ScooterKeyModifiers::NONE,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));

    let state = search_fields_state(&app);
    assert!(
        state.search_debounce_timer.is_none(),
        "cursor move should not schedule a new debounce even when the key includes DirSearchKey"
    );
    let phase = state
        .search_state
        .as_ref()
        .expect("state should be preserved")
        .phase;
    assert!(
        matches!(phase, SearchPhase::Complete { .. }),
        "phase should stay Complete when no re-search is triggered, got {phase:?}"
    );
}

#[tokio::test]
async fn test_changing_replacement_does_not_schedule_search_debounce() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "abc",
        SearchPhase::Complete {
            started,
            completed: started,
        },
        vec![dummy_result()],
    );

    // Move focus from Search to Replace.
    app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Tab,
        ScooterKeyModifiers::NONE,
    ));
    // Typing in the Replace field triggers preview-replacement work, not a new search.
    type_char(&mut app, 'Z');

    let state = search_fields_state(&app);
    assert!(
        state.search_debounce_timer.is_none(),
        "editing the replace field should not schedule a search debounce"
    );
    assert!(
        state.preview_update_state.is_some(),
        "editing the replace field should schedule a preview-replacement debounce"
    );
}

#[tokio::test]
async fn test_back_to_fields_keeps_search_running_until_completion() {
    let started = std::time::Instant::now();
    let mut app = build_test_app_with_phase(
        stdin_source(),
        "foo",
        SearchPhase::Running { started },
        vec![dummy_result()],
    );
    let Screen::SearchFields(state) = &mut app.ui_state.current_screen else {
        panic!("Expected SearchFields screen");
    };
    state.focussed_section = FocussedSection::SearchResults;

    let result = app.handle_key_event(KeyEvent::new(
        ScooterKeyCode::Char('o'),
        ScooterKeyModifiers::CONTROL,
    ));
    assert!(matches!(result, EventHandlingResult::Rerender));

    let state_after_back = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("search state should be preserved");
    assert!(
        !state_after_back.cancelled.load(Ordering::Relaxed),
        "BackToFields should not cancel the active search"
    );
    assert_eq!(
        search_fields_state(&app).focussed_section,
        FocussedSection::SearchFields
    );

    app.handle_background_processing_event(BackgroundProcessingEvent::AddSearchResult(
        SearchResult::new_line(
            Some(PathBuf::from("late.txt")),
            2,
            "late foo".to_owned(),
            LineEnding::Lf,
            true,
        ),
    ));
    let state_after_result = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("search state should still exist");
    assert_eq!(
        state_after_result.results.len(),
        2,
        "results should still append after moving focus back to fields"
    );

    app.handle_background_processing_event(BackgroundProcessingEvent::SearchCompleted);
    let phase_after_complete = search_fields_state(&app)
        .search_state
        .as_ref()
        .expect("search state should still exist")
        .phase;
    assert!(
        matches!(phase_after_complete, SearchPhase::Complete { .. }),
        "search should still complete truthfully after BackToFields, got {phase_after_complete:?}"
    );
}

trait EventHandlingResultExt {
    fn is_rerender(&self) -> bool;
}

impl EventHandlingResultExt for EventHandlingResult {
    fn is_rerender(&self) -> bool {
        matches!(self, EventHandlingResult::Rerender)
    }
}
