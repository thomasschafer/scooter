use bitflags::bitflags;
#[cfg(feature = "steel")]
use steel_derive::Steel;
use unicode_width::UnicodeWidthStr;

use crate::errors::AppError;

#[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
pub enum KeyCode {
    Backspace,
    Char(char),
    Delete,
    End,
    Enter,
    Left,
    Home,
    Right,
}

// Copied from crossterm
bitflags! {
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub struct KeyModifiers: u8 {
        const SHIFT = 0b0000_0001;
        const CONTROL = 0b0000_0010;
        const ALT = 0b0000_0100;
        const SUPER = 0b0000_1000;
        const HYPER = 0b0001_0000;
        const META = 0b0010_0000;
        const NONE = 0b0000_0000;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Field {
    Text(TextField),
    Checkbox(CheckboxField),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldError {
    pub short: String,
    pub long: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "steel", derive(Steel))]
pub struct TextField {
    text: String,
    cursor_idx: usize,
    error: Option<FieldError>,
}

// TODO: treat punctuation as a delimiter to a word, e.g. in "hello,world" deleting the word backwards from
// the end should only delete "world", currently it deletes the whole thing
impl TextField {
    pub fn new(initial: &str) -> Self {
        Self {
            text: initial.to_string(),
            cursor_idx: initial.chars().count(),
            error: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn visual_cursor_pos(&self) -> usize {
        let prefix: String = self.text.chars().take(self.cursor_idx).collect();
        UnicodeWidthStr::width(prefix.as_str())
    }

    pub fn move_cursor_left(&mut self) {
        self.move_cursor_left_by(1);
    }

    pub fn move_cursor_start(&mut self) {
        self.cursor_idx = 0;
    }

    fn move_cursor_left_by(&mut self, n: usize) {
        let cursor_moved_left = self.cursor_idx.saturating_sub(n);
        self.cursor_idx = self.clamp_cursor(cursor_moved_left);
    }

    pub fn move_cursor_right(&mut self) {
        self.move_cursor_right_by(1);
    }

    fn move_cursor_right_by(&mut self, n: usize) {
        let cursor_moved_right = self.cursor_idx.saturating_add(n);
        self.cursor_idx = self.clamp_cursor(cursor_moved_right);
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor_idx = self.text.chars().count();
    }

    pub fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.text.insert(index, new_char);
        self.move_cursor_right();
    }

    fn byte_index(&self) -> usize {
        self.text
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.cursor_idx)
            .unwrap_or(self.text.len())
    }

    pub fn delete_char(&mut self) {
        if self.cursor_idx == 0 {
            return;
        }

        let before_char = self.text.chars().take(self.cursor_idx - 1);
        let after_char = self.text.chars().skip(self.cursor_idx);

        self.text = before_char.chain(after_char).collect();
        self.move_cursor_left();
    }

    pub fn delete_char_forward(&mut self) {
        let before_char = self.text.chars().take(self.cursor_idx);
        let after_char = self.text.chars().skip(self.cursor_idx + 1);

        self.text = before_char.chain(after_char).collect();
    }

    fn previous_word_start(&self) -> usize {
        if self.cursor_idx == 0 {
            return 0;
        }

        let before_char = self.text.chars().take(self.cursor_idx).collect::<Vec<_>>();
        let mut idx = self.cursor_idx - 1;
        while idx > 0 && before_char[idx] == ' ' {
            idx -= 1;
        }
        while idx > 0 && before_char[idx - 1] != ' ' {
            idx -= 1;
        }
        idx
    }

    pub fn move_cursor_back_word(&mut self) {
        self.cursor_idx = self.previous_word_start();
    }

    pub fn delete_word_backward(&mut self) {
        let new_cursor_pos = self.previous_word_start();
        let before_char = self.text.chars().take(new_cursor_pos);
        let after_char = self.text.chars().skip(self.cursor_idx);

        self.text = before_char.chain(after_char).collect();
        self.cursor_idx = new_cursor_pos;
    }

    fn next_word_start(&self) -> usize {
        let after_char = self.text.chars().skip(self.cursor_idx).collect::<Vec<_>>();
        let mut idx = 0;
        let num_chars = after_char.len();
        while idx < num_chars && after_char[idx] != ' ' {
            idx += 1;
        }
        while idx < num_chars && after_char[idx] == ' ' {
            idx += 1;
        }
        self.cursor_idx + idx
    }

    pub fn move_cursor_forward_word(&mut self) {
        self.cursor_idx = self.next_word_start();
    }

    pub fn delete_word_forward(&mut self) {
        let before_char = self.text.chars().take(self.cursor_idx);
        let after_char = self.text.chars().skip(self.next_word_start());

        self.text = before_char.chain(after_char).collect();
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.text.chars().count())
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor_idx = 0;
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor_idx = self.clamp_cursor(text.chars().count());
    }

    pub fn insert_text(&mut self, text: &str) {
        let index = self.byte_index();
        self.text.insert_str(index, text);
        self.cursor_idx += text.chars().count();
    }

    pub fn cursor_idx(&self) -> usize {
        self.cursor_idx
    }

    pub fn set_cursor_idx(&mut self, idx: usize) {
        self.cursor_idx = self.clamp_cursor(idx);
    }

    pub fn delete_to_start(&mut self) {
        self.text = self.text.chars().skip(self.cursor_idx).collect();
        self.cursor_idx = 0;
    }

    pub fn set_error(&mut self, short: String, long: String) {
        self.error = Some(FieldError { short, long });
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    fn handle_keys(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match (code, modifiers) {
            (KeyCode::Char('w'), KeyModifiers::CONTROL)
            | (KeyCode::Backspace, KeyModifiers::ALT) => {
                self.delete_word_backward();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL)
            | (KeyCode::Backspace, KeyModifiers::META) => {
                self.delete_to_start();
            }
            (KeyCode::Backspace, _) => {
                self.delete_char();
            }
            (KeyCode::Left | KeyCode::Char('b' | 'B'), _)
                if modifiers.contains(KeyModifiers::ALT) =>
            {
                self.move_cursor_back_word();
            }
            (KeyCode::Home, _) => {
                self.move_cursor_start();
            }
            (KeyCode::Left, _) => {
                self.move_cursor_left();
            }
            (KeyCode::Right | KeyCode::Char('f' | 'F'), _)
                if modifiers.contains(KeyModifiers::ALT) =>
            {
                self.move_cursor_forward_word();
            }
            (KeyCode::Right, KeyModifiers::META) | (KeyCode::End, _) => {
                self.move_cursor_end();
            }
            (KeyCode::Right, _) => {
                self.move_cursor_right();
            }
            (KeyCode::Char('d') | KeyCode::Delete, KeyModifiers::ALT) => {
                self.delete_word_forward();
            }
            (KeyCode::Delete, _) => {
                self.delete_char_forward();
            }
            (KeyCode::Char(value), _) => {
                self.enter_char(value);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckboxField {
    pub checked: bool,
    pub error: Option<FieldError>, // Not used currently so not rendered
}

impl CheckboxField {
    pub fn new(initial: bool) -> Self {
        Self {
            checked: initial,
            error: None,
        }
    }

    pub fn handle_keys(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if code == KeyCode::Char(' ') {
            self.checked = !self.checked;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldName {
    Search,
    Replace,
    FixedStrings,
    WholeWord,
    MatchCase,
    IncludeFiles,
    ExcludeFiles,
}

impl FieldName {
    pub fn title(&self) -> &str {
        match self {
            FieldName::Search => "Search text",
            FieldName::Replace => "Replace text",
            FieldName::FixedStrings => "Fixed strings",
            FieldName::WholeWord => "Match whole word",
            FieldName::MatchCase => "Match case",
            FieldName::IncludeFiles => "Files to include",
            FieldName::ExcludeFiles => "Files to exclude",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchField {
    pub name: FieldName,
    pub field: Field,
    pub set_by_cli: bool,
}

impl SearchField {
    pub fn new(name: FieldName, field: Field, set_by_cli: bool) -> Self {
        Self {
            name,
            field,
            set_by_cli,
        }
    }
    pub fn new_text(name: FieldName, initial: &str, set_by_cli: bool) -> Self {
        let field = Field::Text(TextField::new(initial));
        Self::new(name, field, set_by_cli)
    }

    pub fn new_checkbox(name: FieldName, initial: bool, set_by_cli: bool) -> Self {
        let field = Field::Checkbox(CheckboxField::new(initial));
        Self::new(name, field, set_by_cli)
    }

    pub fn handle_keys(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        disable_prepopulated_fields: bool,
    ) {
        if self.set_by_cli && disable_prepopulated_fields {
            return;
        }
        self.clear_error();
        match &mut self.field {
            Field::Text(f) => f.handle_keys(code, modifiers),
            Field::Checkbox(f) => f.handle_keys(code, modifiers),
        }
    }

    pub fn cursor_pos(&self) -> Option<usize> {
        match &self.field {
            Field::Text(f) => Some(f.visual_cursor_pos()),
            Field::Checkbox(_) => None,
        }
    }

    pub fn clear_error(&mut self) {
        match &mut self.field {
            Field::Text(f) => f.clear_error(),
            Field::Checkbox(_) => {} // TODO
        }
    }

    pub fn error(&self) -> Option<FieldError> {
        match &self.field {
            Field::Text(f) => f.error.clone(),
            Field::Checkbox(f) => f.error.clone(),
        }
    }
}

pub const NUM_SEARCH_FIELDS: u16 = 7;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchFields {
    pub fields: [SearchField; NUM_SEARCH_FIELDS as usize],
    pub highlighted: usize,
}

macro_rules! define_field_accessor {
    ($method_name:ident, $field_name:expr, $field_variant:ident, $return_type:ty) => {
        pub fn $method_name(&self) -> $return_type {
            let field = self
                .fields
                .iter()
                .find(|SearchField { name, .. }| *name == $field_name)
                .expect("Couldn't find field");

            if let Field::$field_variant(ref inner) = field.field {
                inner
            } else {
                panic!("Incorrect field type")
            }
        }
    };
}

macro_rules! define_field_accessor_mut {
    ($method_name:ident, $field_name:expr, $field_variant:ident, $return_type:ty) => {
        pub fn $method_name(&mut self) -> $return_type {
            let field = self
                .fields
                .iter_mut()
                .find(|SearchField { name, .. }| *name == $field_name)
                .expect("Couldn't find field");

            if let Field::$field_variant(ref mut inner) = &mut field.field {
                inner
            } else {
                panic!("Incorrect field type")
            }
        }
    };
}

impl SearchFields {
    // TODO: generate these automatically?
    define_field_accessor!(search, FieldName::Search, Text, &TextField);
    define_field_accessor!(replace, FieldName::Replace, Text, &TextField);
    define_field_accessor!(
        fixed_strings,
        FieldName::FixedStrings,
        Checkbox,
        &CheckboxField
    );
    define_field_accessor!(whole_word, FieldName::WholeWord, Checkbox, &CheckboxField);
    define_field_accessor!(match_case, FieldName::MatchCase, Checkbox, &CheckboxField);
    define_field_accessor!(include_files, FieldName::IncludeFiles, Text, &TextField);
    define_field_accessor!(exclude_files, FieldName::ExcludeFiles, Text, &TextField);

    define_field_accessor_mut!(search_mut, FieldName::Search, Text, &mut TextField);
    define_field_accessor_mut!(
        include_files_mut,
        FieldName::IncludeFiles,
        Text,
        &mut TextField
    );
    define_field_accessor_mut!(
        exclude_files_mut,
        FieldName::ExcludeFiles,
        Text,
        &mut TextField
    );

    #[allow(clippy::needless_pass_by_value)]
    pub fn with_values(
        search_field_values: &SearchFieldValues<'_>,
        disable_prepopulated_fields: bool,
    ) -> Self {
        let fields = [
            SearchField::new_text(
                FieldName::Search,
                search_field_values.search.value,
                search_field_values.search.set_by_cli,
            ),
            SearchField::new_text(
                FieldName::Replace,
                search_field_values.replace.value,
                search_field_values.replace.set_by_cli,
            ),
            SearchField::new_checkbox(
                FieldName::FixedStrings,
                search_field_values.fixed_strings.value,
                search_field_values.fixed_strings.set_by_cli,
            ),
            SearchField::new_checkbox(
                FieldName::WholeWord,
                search_field_values.match_whole_word.value,
                search_field_values.match_whole_word.set_by_cli,
            ),
            SearchField::new_checkbox(
                FieldName::MatchCase,
                search_field_values.match_case.value,
                search_field_values.match_case.set_by_cli,
            ),
            SearchField::new_text(
                FieldName::IncludeFiles,
                search_field_values.include_files.value,
                search_field_values.include_files.set_by_cli,
            ),
            SearchField::new_text(
                FieldName::ExcludeFiles,
                search_field_values.exclude_files.value,
                search_field_values.exclude_files.set_by_cli,
            ),
        ];

        Self {
            highlighted: Self::initial_highlight_position(&fields, disable_prepopulated_fields),
            fields,
        }
    }

    fn initial_highlight_position(
        fields: &[SearchField],
        disable_prepopulated_fields: bool,
    ) -> usize {
        if disable_prepopulated_fields {
            fields
                .iter()
                .enumerate()
                .find_map(|(idx, field)| if !field.set_by_cli { Some(idx) } else { None })
                .unwrap_or(0)
        } else {
            0
        }
    }

    pub fn highlighted_field(&self) -> &SearchField {
        &self.fields[self.highlighted]
    }

    pub fn highlighted_field_mut(&mut self) -> &mut SearchField {
        &mut self.fields[self.highlighted]
    }

    fn focus_impl(&mut self, backward: bool, disable_prepopulated_fields: bool) {
        let step = if backward {
            self.fields.len().saturating_sub(1)
        } else {
            1
        };

        let initial = self.highlighted;
        let mut next = (initial + step).rem_euclid(self.fields.len());
        if disable_prepopulated_fields {
            while self.fields[next].set_by_cli && next != initial {
                next = (next + step).rem_euclid(self.fields.len());
            }
        }
        self.highlighted = next;
    }

    pub fn focus_next(&mut self, disable_prepopulated_fields: bool) {
        self.focus_impl(false, disable_prepopulated_fields);
    }

    pub fn focus_prev(&mut self, disable_prepopulated_fields: bool) {
        self.focus_impl(true, disable_prepopulated_fields);
    }

    pub fn errors(&self) -> Vec<AppError> {
        self.fields
            .iter()
            .filter_map(|field| {
                field.error().map(|err| AppError {
                    name: field.name.title().to_string(),
                    long: err.long,
                })
            })
            .collect::<Vec<_>>()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldValue<T> {
    pub value: T,
    pub set_by_cli: bool,
}

impl<T> FieldValue<T> {
    pub fn new(value: T, set_by_cli: bool) -> Self {
        Self { value, set_by_cli }
    }
}

impl<T: Default> Default for FieldValue<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            set_by_cli: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchFieldValues<'a> {
    pub search: FieldValue<&'a str>,
    pub replace: FieldValue<&'a str>,
    pub fixed_strings: FieldValue<bool>,
    pub match_whole_word: FieldValue<bool>,
    pub match_case: FieldValue<bool>,
    pub include_files: FieldValue<&'a str>,
    pub exclude_files: FieldValue<&'a str>,
}

impl<'a> Default for SearchFieldValues<'a> {
    fn default() -> SearchFieldValues<'a> {
        Self {
            search: FieldValue::new(Self::DEFAULT_SEARCH, false),
            replace: FieldValue::new(Self::DEFAULT_REPLACE, false),
            fixed_strings: FieldValue::new(Self::DEFAULT_FIXED_STRINGS, false),
            match_whole_word: FieldValue::new(Self::DEFAULT_WHOLE_WORD, false),
            match_case: FieldValue::new(Self::DEFAULT_MATCH_CASE, false),
            include_files: FieldValue::new(Self::DEFAULT_INCLUDE_FILES, false),
            exclude_files: FieldValue::new(Self::DEFAULT_EXCLUDE_FILES, false),
        }
    }
}

impl SearchFieldValues<'_> {
    const DEFAULT_SEARCH: &'static str = "";
    const DEFAULT_REPLACE: &'static str = "";
    const DEFAULT_FIXED_STRINGS: bool = false;
    const DEFAULT_WHOLE_WORD: bool = false;
    const DEFAULT_MATCH_CASE: bool = true;
    const DEFAULT_INCLUDE_FILES: &'static str = "";
    const DEFAULT_EXCLUDE_FILES: &'static str = "";
}

#[cfg(test)]
mod tests {
    use crate::fields::TextField;

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
    fn test_unicode_text_handling() {
        let mut field = TextField::new("");

        // Test emoji input
        field.enter_char('👍');
        assert_eq!(field.text(), "👍");
        assert_eq!(field.cursor_idx, 1);

        field.enter_char('🎉');
        assert_eq!(field.text(), "👍🎉");
        assert_eq!(field.cursor_idx, 2);

        // Test multi-byte characters
        field.clear();
        for c in "こんにちは".chars() {
            field.enter_char(c);
        }
        assert_eq!(field.text(), "こんにちは");
        assert_eq!(field.cursor_idx, 5);

        // Test moving through multi-byte characters
        field.move_cursor_left();
        assert_eq!(field.cursor_idx, 4);
        field.delete_char();
        assert_eq!(field.text(), "こんには");

        // Test mixed ASCII and Unicode
        field.clear();
        for c in "Hello👋World🌍".chars() {
            field.enter_char(c);
        }
        assert_eq!(field.text(), "Hello👋World🌍");
        assert_eq!(field.cursor_idx, 12);

        // Move cursor to before emoji
        field.move_cursor_start();
        for _ in 0..5 {
            field.move_cursor_right();
        }
        assert_eq!(field.cursor_idx, 5);
        field.delete_char_forward();
        assert_eq!(field.text(), "HelloWorld🌍");
    }

    #[test]
    fn test_empty_text_edge_cases() {
        let mut field = TextField::new("");

        // Test operations on empty field
        assert_eq!(field.text(), "");
        assert_eq!(field.cursor_idx, 0);

        field.delete_char();
        assert_eq!(field.text(), "");
        assert_eq!(field.cursor_idx, 0);

        field.delete_char_forward();
        assert_eq!(field.text(), "");
        assert_eq!(field.cursor_idx, 0);

        field.move_cursor_left();
        assert_eq!(field.cursor_idx, 0);

        field.move_cursor_right();
        assert_eq!(field.cursor_idx, 0);

        field.delete_word_backward();
        assert_eq!(field.text(), "");

        field.delete_word_forward();
        assert_eq!(field.text(), "");

        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 0);

        field.move_cursor_back_word();
        assert_eq!(field.cursor_idx, 0);
    }

    #[test]
    fn test_cursor_boundary_cases() {
        let mut field = TextField::new("Test");

        // Test cursor at start boundary
        field.move_cursor_start();
        field.move_cursor_left();
        assert_eq!(field.cursor_idx, 0);

        field.delete_char();
        assert_eq!(field.text(), "Test");
        assert_eq!(field.cursor_idx, 0);

        // Test cursor at end boundary
        field.move_cursor_end();
        field.move_cursor_right();
        assert_eq!(field.cursor_idx, 4);

        field.delete_char_forward();
        assert_eq!(field.text(), "Test");
        assert_eq!(field.cursor_idx, 4);

        // Test moving by multiple positions
        field.move_cursor_left_by(10);
        assert_eq!(field.cursor_idx, 0);

        field.move_cursor_right_by(10);
        assert_eq!(field.cursor_idx, 4);
    }

    #[test]
    fn test_word_navigation_with_punctuation() {
        let mut field = TextField::new("");

        // Test with punctuation
        for c in "hello, world! foo-bar_baz.qux".chars() {
            field.enter_char(c);
        }

        field.move_cursor_start();
        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 7); // After "hello, " (punctuation is part of word)

        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 14); // After "world! "

        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 29); // End of string

        field.move_cursor_back_word();
        assert_eq!(field.cursor_idx, 14); // Start of "foo-bar_baz.qux"

        field.move_cursor_back_word();
        assert_eq!(field.cursor_idx, 7); // Start of "world!"

        // Test word deletion with punctuation
        field.delete_word_forward();
        assert_eq!(field.text(), "hello, foo-bar_baz.qux");

        field.move_cursor_end();
        field.delete_word_backward();
        assert_eq!(field.text(), "hello, ");
    }

    #[test]
    fn test_word_navigation_special_cases() {
        let mut field = TextField::new("");

        // Multiple spaces
        for c in "hello     world".chars() {
            field.enter_char(c);
        }
        field.move_cursor_start();
        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 10); // Should skip all spaces

        // Leading/trailing spaces
        field.clear();
        for c in "   hello   ".chars() {
            field.enter_char(c);
        }
        field.move_cursor_start();
        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 3); // After "   " (skips leading spaces to "hello")

        field.move_cursor_end();
        field.move_cursor_back_word();
        assert_eq!(field.cursor_idx, 3); // Start of "hello"

        // Only spaces
        field.clear();
        for c in "     ".chars() {
            field.enter_char(c);
        }
        field.move_cursor_start();
        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 5);

        field.move_cursor_back_word();
        assert_eq!(field.cursor_idx, 0);
    }

    #[test]
    fn test_visual_cursor_position() {
        let mut field = TextField::new("");

        // ASCII text
        for c in "Hello".chars() {
            field.enter_char(c);
        }
        assert_eq!(field.visual_cursor_pos(), 5);

        // Wide characters (e.g., East Asian characters)
        field.clear();
        for c in "你好".chars() {
            field.enter_char(c);
        }
        assert_eq!(field.visual_cursor_pos(), 4); // Each character is 2 columns wide

        // Mixed width characters
        field.clear();
        for c in "Hi你好".chars() {
            field.enter_char(c);
        }
        assert_eq!(field.visual_cursor_pos(), 6); // "Hi" = 2, "你好" = 4

        // Emoji (usually 2 columns wide)
        field.clear();
        field.enter_char('😀');
        assert_eq!(field.visual_cursor_pos(), 2);

        // Zero-width characters
        field.clear();
        for c in "e\u{0301}".chars() {
            // e with combining acute accent
            field.enter_char(c);
        }
        assert_eq!(field.visual_cursor_pos(), 1); // Combining character has 0 width
    }

    #[test]
    fn test_cursor_position_with_wide_chars() {
        let mut field = TextField::new("");

        // Insert mixed-width text
        for c in "A你B好C".chars() {
            field.enter_char(c);
        }

        // Test cursor movement
        field.move_cursor_start();
        assert_eq!(field.cursor_idx, 0);
        assert_eq!(field.visual_cursor_pos(), 0);

        field.move_cursor_right();
        assert_eq!(field.cursor_idx, 1); // After 'A'
        assert_eq!(field.visual_cursor_pos(), 1);

        field.move_cursor_right();
        assert_eq!(field.cursor_idx, 2); // After '你'
        assert_eq!(field.visual_cursor_pos(), 3); // 1 + 2

        field.move_cursor_right();
        assert_eq!(field.cursor_idx, 3); // After 'B'
        assert_eq!(field.visual_cursor_pos(), 4); // 1 + 2 + 1

        // Test deletion with wide characters
        field.delete_char();
        assert_eq!(field.text(), "A你好C");
        assert_eq!(field.cursor_idx, 2);

        field.delete_char();
        assert_eq!(field.text(), "A好C");
        assert_eq!(field.cursor_idx, 1);
    }

    #[test]
    fn test_set_text_and_insert_text() {
        let mut field = TextField::new("initial");

        // Test set_text
        field.set_text("new text");
        assert_eq!(field.text(), "new text");
        assert_eq!(field.cursor_idx, 8);

        // Test set_text with cursor clamping
        field.set_cursor_idx(100);
        assert_eq!(field.cursor_idx, 8);

        field.set_text("short");
        assert_eq!(field.text(), "short");
        assert_eq!(field.cursor_idx, 5); // Cursor should be clamped

        // Test insert_text
        field.move_cursor_start();
        field.insert_text("pre-");
        assert_eq!(field.text(), "pre-short");
        assert_eq!(field.cursor_idx, 4);

        field.move_cursor_end();
        field.insert_text("-post");
        assert_eq!(field.text(), "pre-short-post");
        assert_eq!(field.cursor_idx, 14);

        // Insert in middle
        field.set_cursor_idx(9); // After "pre-short"
        field.insert_text("[mid]");
        assert_eq!(field.text(), "pre-short[mid]-post");
        assert_eq!(field.cursor_idx, 14); // 9 + 5
    }

    #[test]
    fn test_delete_to_start() {
        let mut field = TextField::new("Hello World");

        field.set_cursor_idx(6); // After "Hello "
        field.delete_to_start();
        assert_eq!(field.text(), "World");
        assert_eq!(field.cursor_idx, 0);

        // Delete from end
        field.set_text("Test");
        field.move_cursor_end();
        field.delete_to_start();
        assert_eq!(field.text(), "");
        assert_eq!(field.cursor_idx, 0);

        // Delete from start (no-op)
        field.set_text("Test");
        field.move_cursor_start();
        field.delete_to_start();
        assert_eq!(field.text(), "Test");
        assert_eq!(field.cursor_idx, 0);
    }

    #[test]
    fn test_complex_unicode_scenarios() {
        let mut field = TextField::new("");

        // Test with various Unicode categories
        let test_string = "café☕ 世界🌏 Здравствуй👋";
        for c in test_string.chars() {
            field.enter_char(c);
        }
        assert_eq!(field.text(), test_string);

        // Navigate through the text
        field.move_cursor_start();
        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 6); // After "café☕ "

        field.move_cursor_forward_word();
        assert_eq!(field.cursor_idx, 10); // After "世界🌏 "

        // Delete operations
        field.delete_word_backward();
        assert_eq!(field.text(), "café☕ Здравствуй👋");

        // Test cursor index vs visual position
        field.move_cursor_start();
        for _ in 0..4 {
            field.move_cursor_right();
        }
        assert_eq!(field.cursor_idx, 4); // After "café"
        assert_eq!(field.visual_cursor_pos(), 4); // café has no wide chars

        field.move_cursor_right(); // Move past ☕
        assert_eq!(field.cursor_idx, 5);
        assert_eq!(field.visual_cursor_pos(), 6); // 4 + 2 for emoji
    }

    #[test]
    fn test_byte_index_calculation() {
        let mut field = TextField::new("a你b");

        // Verify byte indices for mixed content
        field.set_cursor_idx(0);
        assert_eq!(field.byte_index(), 0);

        field.set_cursor_idx(1); // After 'a'
        assert_eq!(field.byte_index(), 1);

        field.set_cursor_idx(2); // After '你'
        assert_eq!(field.byte_index(), 4); // 'a' = 1 byte, '你' = 3 bytes

        field.set_cursor_idx(3); // After 'b'
        assert_eq!(field.byte_index(), 5);

        // Test at end
        field.set_cursor_idx(10); // Beyond end
        assert_eq!(field.cursor_idx, 3); // Clamped
        assert_eq!(field.byte_index(), 5);
    }
}
