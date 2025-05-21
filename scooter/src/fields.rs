use ratatui::{
    crossterm::event::{KeyCode, KeyModifiers},
    text::Text,
};

#[derive(Clone, Debug)]
pub struct FieldError {
    pub short: String,
    pub long: String,
}

#[derive(Default)]
pub struct TextField {
    pub text: String,
    pub cursor_idx: usize,
    pub error: Option<FieldError>,
}

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

    pub fn cursor_pos(&self) -> usize {
        let prefix: String = self.text.chars().take(self.cursor_idx).collect();
        Text::from(prefix).width()
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

    fn byte_index(&mut self) -> usize {
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
                self.clear();
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
            (_, _) => {}
        }
    }
}

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

pub enum Field {
    Text(TextField),
    Checkbox(CheckboxField),
}

impl Field {
    pub fn text(initial: &str) -> Field {
        Field::Text(TextField::new(initial))
    }

    pub fn checkbox(initial: bool) -> Field {
        Field::Checkbox(CheckboxField::new(initial))
    }

    pub fn handle_keys(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.clear_error();
        match self {
            Field::Text(f) => f.handle_keys(code, modifiers),
            Field::Checkbox(f) => f.handle_keys(code, modifiers),
        }
    }

    pub fn cursor_pos(&self) -> Option<usize> {
        match self {
            Field::Text(f) => Some(f.cursor_pos()),
            Field::Checkbox(_) => None,
        }
    }

    pub fn clear_error(&mut self) {
        match self {
            Field::Text(f) => f.clear_error(),
            Field::Checkbox(_) => {} // TODO
        }
    }

    pub fn error(&self) -> Option<FieldError> {
        match self {
            Field::Text(f) => f.error.clone(),
            Field::Checkbox(f) => f.error.clone(),
        }
    }
}
