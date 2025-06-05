use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use scooter::{CheckboxField, FieldName, FieldValue, SearchFieldValues, SearchFields, TextField};

#[test]
fn test_text_field_operations() {
    let mut field = TextField::new("");

    // Test input
    for c in "Hello".chars() {
        field.enter_char(c);
    }
    assert_eq!(field.text(), "Hello");
    assert_eq!(field.cursor_idx, 5);

    field.move_cursor_left();
    assert_eq!(field.cursor_idx, 4);
    field.move_cursor_right();
    assert_eq!(field.cursor_idx, 5);
    field.move_cursor_start();
    assert_eq!(field.cursor_idx, 0);
    field.move_cursor_end();
    assert_eq!(field.cursor_idx, 5);

    field.clear();
    for c in "Hello world".chars() {
        field.enter_char(c);
    }
    field.move_cursor_start();
    field.move_cursor_forward_word();
    assert_eq!(field.cursor_idx, 6);
    field.move_cursor_forward_word();
    assert_eq!(field.cursor_idx, 11);
    field.move_cursor_forward_word();
    assert_eq!(field.cursor_idx, 11);
    field.move_cursor_back_word();
    assert_eq!(field.cursor_idx, 6);

    // Test deletion
    field.move_cursor_start();
    field.delete_char_forward();
    assert_eq!(field.text(), "ello world");
    field.move_cursor_end();
    field.delete_char();
    assert_eq!(field.text(), "ello worl");
    field.move_cursor_start();
    field.delete_word_forward();
    assert_eq!(field.text(), "worl");
    field.move_cursor_end();
    field.delete_word_backward();
    assert_eq!(field.text(), "");
}

#[test]
fn test_checkbox_field() {
    let mut field = CheckboxField::new(false);
    assert!(!field.checked);

    field.handle_keys(KeyCode::Char(' '), KeyModifiers::empty());
    assert!(field.checked);

    field.handle_keys(KeyCode::Char(' '), KeyModifiers::empty());
    assert!(!field.checked);

    field.handle_keys(KeyCode::Enter, KeyModifiers::empty());
    assert!(!field.checked);
}

#[test]
fn test_search_fields() {
    let mut search_fields = SearchFields::with_values(&SearchFieldValues::default(), true);

    // Test focus navigation
    assert_eq!(search_fields.highlighted, 0);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 1);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 2);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 3);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 4);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 5);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 6);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 0);
    search_fields.focus_prev(true);
    assert_eq!(search_fields.highlighted, 6);
    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 0);

    for c in "test search".chars() {
        search_fields.highlighted_field_mut().handle_keys(
            KeyCode::Char(c),
            KeyModifiers::NONE,
            true,
        );
    }
    assert_eq!(search_fields.search().text, "test search");

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 1);
    for c in "test replace".chars() {
        search_fields.highlighted_field_mut().handle_keys(
            KeyCode::Char(c),
            KeyModifiers::NONE,
            true,
        );
    }
    assert_eq!(search_fields.replace().text, "test replace");

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 2);
    search_fields
        .highlighted_field_mut()
        .handle_keys(KeyCode::Char(' '), KeyModifiers::NONE, true);
    assert!(search_fields.fixed_strings().checked);

    assert_eq!(search_fields.search().text, "test search");
    assert_eq!(search_fields.fixed_strings().checked, true);

    search_fields
        .highlighted_field_mut()
        .handle_keys(KeyCode::Char(' '), KeyModifiers::NONE, true);

    assert_eq!(search_fields.search().text, "test search");
    assert_eq!(search_fields.fixed_strings().checked, false);
}

#[test]
fn test_focus_with_locked_disabled_fields() {
    let mut search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new("prepopulated", true),
            replace: FieldValue::new("", false),
            fixed_strings: FieldValue::new(true, true),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("*.rs", true),
            exclude_files: FieldValue::new("", false),
        },
        true,
    );

    assert_eq!(search_fields.highlighted, 1);
    assert_eq!(search_fields.highlighted_field().name, FieldName::Replace);

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 3);
    assert_eq!(search_fields.highlighted_field().name, FieldName::WholeWord);

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 4);
    assert_eq!(search_fields.highlighted_field().name, FieldName::MatchCase);

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 6);
    assert_eq!(
        search_fields.highlighted_field().name,
        FieldName::ExcludeFiles
    );

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 1);
    assert_eq!(search_fields.highlighted_field().name, FieldName::Replace);

    search_fields.focus_prev(true);
    assert_eq!(search_fields.highlighted, 6);
    assert_eq!(
        search_fields.highlighted_field().name,
        FieldName::ExcludeFiles
    );

    search_fields.focus_prev(true);
    assert_eq!(search_fields.highlighted, 4);
    assert_eq!(search_fields.highlighted_field().name, FieldName::MatchCase);
}

#[test]
fn test_focus_with_unlocked_disabled_fields() {
    let mut search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new("prepopulated", true),
            replace: FieldValue::new("", false),
            fixed_strings: FieldValue::new(true, true),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("*.rs", true),
            exclude_files: FieldValue::new("", false),
        },
        false,
    );

    assert_eq!(search_fields.highlighted, 0);

    search_fields.focus_next(false);
    assert_eq!(search_fields.highlighted, 1);
    search_fields.focus_next(false);
    assert_eq!(search_fields.highlighted, 2);
    search_fields.focus_next(false);
    assert_eq!(search_fields.highlighted, 3);
}

#[test]
fn test_focus_all_fields_disabled_and_locked() {
    let mut search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new("search", true),
            replace: FieldValue::new("replace", true),
            fixed_strings: FieldValue::new(true, true),
            match_whole_word: FieldValue::new(false, true),
            match_case: FieldValue::new(true, true),
            include_files: FieldValue::new("*.rs", true),
            exclude_files: FieldValue::new("*.txt", true),
        },
        true,
    );

    assert_eq!(search_fields.highlighted, 0);

    search_fields.focus_next(true);
    assert_eq!(search_fields.highlighted, 0);

    search_fields.focus_prev(true);
    assert_eq!(search_fields.highlighted, 0);
}

#[test]
fn test_initial_highlight_position_with_prepopulated_fields_disable_true() {
    let search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new("cli_search", true),
            replace: FieldValue::new("cli_replace", true),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("*.rs", true),
            exclude_files: FieldValue::new("", false),
        },
        true,
    );

    assert_eq!(search_fields.highlighted, 2);
    assert_eq!(
        search_fields.highlighted_field().name,
        FieldName::FixedStrings
    );
}

#[test]
fn test_initial_highlight_position_with_prepopulated_fields_disable_false() {
    let search_fields = SearchFields::with_values(
        &SearchFieldValues {
            search: FieldValue::new("cli_search", true),
            replace: FieldValue::new("cli_replace", true),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("*.rs", true),
            exclude_files: FieldValue::new("", false),
        },
        false,
    );

    assert_eq!(search_fields.highlighted, 0);
    assert_eq!(search_fields.highlighted_field().name, FieldName::Search);
}
