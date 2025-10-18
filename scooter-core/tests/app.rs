use frep_core::{
    line_reader::LineEnding,
    replace::ReplaceResult,
    search::{SearchResult, SearchResultWithReplacement},
};
use insta::assert_debug_snapshot;
use scooter_core::{
    app::{EventHandlingResult, InputSource},
    errors::AppError,
    fields::{FieldValue, SearchFieldValues, SearchFields},
    replace::{PerformingReplacementState, ReplaceState},
};
use std::{
    env::current_dir,
    mem,
    path::PathBuf,
    sync::{atomic::AtomicBool, atomic::AtomicUsize, Arc},
};
use tokio::sync::mpsc;

use scooter_core::{
    app::{App, AppRunConfig, FocussedSection, Popup, Screen, SearchFieldsState, SearchState},
    fields::{KeyCode as ScooterKeyCode, KeyModifiers as ScooterKeyModifiers},
};

#[tokio::test]
async fn test_replace_state() {
    let mut state = ReplaceState {
        num_successes: 2,
        num_ignored: 1,
        errors: (1..3)
            .map(|n| SearchResultWithReplacement {
                search_result: SearchResult {
                    path: Some(PathBuf::from(format!("error-{n}.txt"))),
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
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
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
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
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

    let res = app.handle_key_event(ScooterKeyCode::Char('o'), ScooterKeyModifiers::CONTROL);
    assert!(!matches!(res, EventHandlingResult::Exit(None)));
    assert_eq!(app.search_fields.search().text(), "foo");
    assert_eq!(app.search_fields.replace().text(), "bar");
    assert!(app.search_fields.fixed_strings().checked);
    assert_eq!(app.search_fields.include_files().text(), "pattern");
    assert_eq!(app.search_fields.exclude_files().text(), "");
    assert!(matches!(app.current_screen, Screen::SearchFields(_)));
}

fn test_error_popup_invalid_input_impl(search_fields: &SearchFieldValues<'_>) {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        InputSource::Directory(current_dir().unwrap()),
        search_fields,
        &AppRunConfig::default(),
        true,
        false,
    );

    // Simulate search being triggered in background
    let res = app.perform_search_if_valid();
    assert!(!matches!(res, EventHandlingResult::Exit(None)));
    assert!(app.popup().is_none());

    // Hitting enter should show popup
    app.handle_key_event(ScooterKeyCode::Enter, ScooterKeyModifiers::NONE);
    assert!(matches!(app.current_screen, Screen::SearchFields(_)));
    assert!(matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(ScooterKeyCode::Esc, ScooterKeyModifiers::NONE);
    assert!(!matches!(res, EventHandlingResult::Exit(None)));
    assert!(!matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(ScooterKeyCode::Esc, ScooterKeyModifiers::NONE);
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
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
    );
    let screen_variant = std::mem::discriminant(&initial_screen);
    app.current_screen = initial_screen;

    assert!(app.popup().is_none());
    assert_eq!(mem::discriminant(&app.current_screen), screen_variant);

    let res_open = app.handle_key_event(ScooterKeyCode::Char('h'), ScooterKeyModifiers::CONTROL);
    assert!(matches!(res_open, EventHandlingResult::Rerender));
    assert!(matches!(app.popup(), Some(Popup::Help)));
    assert_eq!(std::mem::discriminant(&app.current_screen), screen_variant);

    let res_close = app.handle_key_event(ScooterKeyCode::Esc, ScooterKeyModifiers::NONE);
    assert!(matches!(res_close, EventHandlingResult::Rerender));
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

#[tokio::test]
async fn test_keymaps_search_fields() {
    let (app, _app_event_receiver) = App::new_with_receiver(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
    );

    assert!(matches!(app.current_screen, Screen::SearchFields(_)));

    assert_debug_snapshot!("search_fields_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_fields_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_complete() {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
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
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
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
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
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
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
    );

    let replace_state_with_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from("error.txt")),
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
        InputSource::Directory(current_dir().unwrap()),
        &SearchFieldValues::default(),
        &AppRunConfig::default(),
        true,
        false,
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
        InputSource::Directory(current_dir().unwrap()),
        &search_field_values,
        &AppRunConfig::default(),
        true,
        false,
    );

    assert_eq!(
        app.search_fields
            .fields
            .iter()
            .map(|f| f.set_by_cli)
            .collect::<Vec<_>>(),
        vec![true, false, false, false, false, true, false]
    );

    let result = app.handle_key_event(ScooterKeyCode::Char('u'), ScooterKeyModifiers::ALT);
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

    let (mut app, _app_event_receiver) = App::new_with_receiver(
        InputSource::Directory(current_dir().unwrap()),
        &search_field_values,
        &AppRunConfig::default(),
        true,
        false,
    );

    assert_eq!(app.search_fields.highlighted, 2);

    app.handle_key_event(ScooterKeyCode::Tab, ScooterKeyModifiers::NONE);
    assert_eq!(app.search_fields.highlighted, 3);

    app.handle_key_event(ScooterKeyCode::Tab, ScooterKeyModifiers::NONE);
    assert_eq!(app.search_fields.highlighted, 4);

    app.handle_key_event(ScooterKeyCode::Tab, ScooterKeyModifiers::NONE);
    assert_eq!(app.search_fields.highlighted, 6);

    app.handle_key_event(ScooterKeyCode::BackTab, ScooterKeyModifiers::NONE);
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
        InputSource::Directory(current_dir().unwrap()),
        &search_field_values,
        &AppRunConfig::default(),
        true,
        false,
    );

    for field in &app.search_fields.fields {
        assert!(field.set_by_cli);
    }

    app.handle_key_event(ScooterKeyCode::Char('x'), ScooterKeyModifiers::NONE);
    assert_eq!(app.search_fields.search().text(), "search"); // Shouldn't have changed - all fields locked

    app.handle_key_event(ScooterKeyCode::Char('u'), ScooterKeyModifiers::ALT);

    for field in &app.search_fields.fields {
        assert!(!field.set_by_cli);
    }

    app.handle_key_event(ScooterKeyCode::Char('x'), ScooterKeyModifiers::NONE);
    assert_eq!(app.search_fields.search().text(), "searchx");
}
