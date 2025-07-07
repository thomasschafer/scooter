#[cfg(feature = "steel")]
use steel_derive::Steel;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "steel", derive(Steel))]
pub struct TextField {
    text: String,
    cursor_idx: usize,
}

// TODO: treat punctuation as a delimiter to a word, e.g. in "hello,world" deleting the word backwards from
// the end should only delete "world", currently it deletes the whole thing
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
