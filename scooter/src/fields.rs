use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::iter::Iterator;

use scooter_core::fields::TextField as CoreTextField;

use crate::app::AppError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldError {
    pub short: String,
    pub long: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextField {
    core_text_field: CoreTextField,
    error: Option<FieldError>,
}

impl TextField {
    pub fn new(initial: &str) -> Self {
        Self {
            core_text_field: CoreTextField::new(initial),
            error: None,
        }
    }

    pub fn text(&self) -> &str {
        self.core_text_field.text()
    }

    pub fn cursor_pos(&self) -> usize {
        self.core_text_field.visual_cursor_pos()
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
                self.core_text_field.delete_word_backward();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL)
            | (KeyCode::Backspace, KeyModifiers::META) => {
                self.core_text_field.delete_line_backwards();
            }
            (KeyCode::Backspace, _) => {
                self.core_text_field.delete_char();
            }
            (KeyCode::Left | KeyCode::Char('b' | 'B'), _)
                if modifiers.contains(KeyModifiers::ALT) =>
            {
                self.core_text_field.move_cursor_back_word();
            }
            (KeyCode::Home, _) => {
                self.core_text_field.move_cursor_start();
            }
            (KeyCode::Left, _) => {
                self.core_text_field.move_cursor_left();
            }
            (KeyCode::Right | KeyCode::Char('f' | 'F'), _)
                if modifiers.contains(KeyModifiers::ALT) =>
            {
                self.core_text_field.move_cursor_forward_word();
            }
            (KeyCode::Right, KeyModifiers::META) | (KeyCode::End, _) => {
                self.core_text_field.move_cursor_end();
            }
            (KeyCode::Right, _) => {
                self.core_text_field.move_cursor_right();
            }
            (KeyCode::Char('d') | KeyCode::Delete, KeyModifiers::ALT) => {
                self.core_text_field.delete_word_forward();
            }
            (KeyCode::Delete, _) => {
                self.core_text_field.delete_char_forward();
            }
            (KeyCode::Char(value), _) => {
                self.core_text_field.enter_char(value);
            }
            (_, _) => {}
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
pub enum Field {
    Text(TextField),
    Checkbox(CheckboxField),
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
    pub(crate) fn title(&self) -> &str {
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
            Field::Text(f) => Some(f.cursor_pos()),
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

pub const NUM_SEARCH_FIELDS: usize = 7;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchFields {
    pub fields: [SearchField; NUM_SEARCH_FIELDS],
    pub highlighted: usize,
    pub advanced_regex: bool,
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
            advanced_regex: false,
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

    pub fn with_advanced_regex(mut self, advanced_regex: bool) -> Self {
        self.advanced_regex = advanced_regex;
        self
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
