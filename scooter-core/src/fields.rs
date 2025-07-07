#[cfg(feature = "steel")]
use steel_derive::Steel;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "steel", derive(Steel))]
pub struct TextField {
    text: String,
    cursor_idx: usize,
}

impl TextField {
    pub fn new(initial: &str) -> Self {
        Self {
            text: initial.to_string(),
            cursor_idx: initial.chars().count(),
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
        self.cursor_idx = self.text.chars().count().min(self.cursor_idx);
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
}

// TODO: add more tests
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
}
