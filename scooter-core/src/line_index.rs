//! Conversion between byte offsets and line numbers in text.
use std::ops::Range;

use crate::{line_reader::LineEnding, search::Line};

/// Returns the byte positions of each `\n` in `bytes`, in ascending order
pub(crate) fn newline_positions(bytes: &[u8]) -> Vec<usize> {
    memchr::memchr_iter(b'\n', bytes).collect()
}

/// Helper struct to efficiently convert byte offsets to line numbers and extract lines
pub(crate) struct LineIndex<'a> {
    content: &'a str,
    /// Byte positions of newline characters
    newline_positions: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(content: &'a str) -> Self {
        Self {
            content,
            newline_positions: newline_positions(content.as_bytes()),
        }
    }

    /// The text being indexed
    pub(crate) fn content(&self) -> &'a str {
        self.content
    }

    /// Get line number (1-indexed) for a byte offset
    pub(crate) fn line_number_at(&self, byte_offset: usize) -> usize {
        // Binary search to find how many newlines come before this offset
        // Both Ok and Err return the same value: the number of newlines before/at this position + 1.
        // If `byte_offset` lands on a '\n', we treat it as part of the line it terminates.
        match self.newline_positions.binary_search(&byte_offset) {
            Ok(idx) | Err(idx) => idx + 1,
        }
    }

    /// Get the byte offset where a line starts (`line_num` is 1-indexed)
    pub(crate) fn line_start_byte(&self, line_num: usize) -> usize {
        assert!(line_num >= 1, "Line numbers are 1-indexed");
        if line_num == 1 {
            0
        } else {
            // Line N starts after the (N-1)th newline
            self.newline_positions[line_num - 2] + 1
        }
    }

    /// Get the byte range of a line (`line_num` is 1-indexed), including any `\r` of a `CrLf`
    /// line ending but excluding the `\n`
    pub(crate) fn line_span(&self, line_num: usize) -> Range<usize> {
        assert!(line_num >= 1, "Line numbers are 1-indexed");
        let end = self
            .newline_positions
            .get(line_num - 1)
            .copied()
            .unwrap_or(self.content.len());
        self.line_start_byte(line_num)..end
    }

    /// Get the line ending of a line (`line_num` is 1-indexed)
    pub(crate) fn line_ending(&self, line_num: usize) -> LineEnding {
        assert!(line_num >= 1, "Line numbers are 1-indexed");
        match self.newline_positions.get(line_num - 1) {
            Some(&newline_pos)
                if newline_pos > 0 && self.content.as_bytes()[newline_pos - 1] == b'\r' =>
            {
                LineEnding::CrLf
            }
            Some(_) => LineEnding::Lf,
            None => LineEnding::None,
        }
    }

    /// Get the byte offset where a line ends (exclusive of line ending).
    /// For `CrLf` lines this excludes the `\r`, matching `BufReadExt::lines_with_endings` behaviour.
    pub(crate) fn line_end_byte(&self, line_num: usize) -> usize {
        let span_end = self.line_span(line_num).end;
        match self.line_ending(line_num) {
            LineEnding::CrLf => span_end - 1,
            LineEnding::Lf | LineEnding::None => span_end,
        }
    }

    /// Returns the number of `\n` characters in the content
    pub(crate) fn newline_count(&self) -> usize {
        self.newline_positions.len()
    }

    /// Returns the total number of lines in the content
    pub(crate) fn total_lines(&self) -> usize {
        // Number of newlines + 1, unless the file is empty
        if self.content.is_empty() {
            0
        } else {
            self.newline_positions.len() + 1
        }
    }

    /// Extract full lines from `start_line` to `end_line` (both 1-indexed, inclusive)
    pub(crate) fn extract_lines(&self, start_line: usize, end_line: usize) -> Vec<(usize, Line)> {
        assert!(start_line >= 1, "Line numbers are 1-indexed");
        assert!(start_line <= end_line, "start_line must be <= end_line");

        (start_line..=end_line)
            .map(|line_num| {
                let start = self.line_start_byte(line_num);
                let end = self.line_end_byte(line_num);
                let content = self.content[start..end].to_string();
                let line_ending = self.line_ending(line_num);

                (
                    line_num,
                    Line {
                        content,
                        line_ending,
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_index_single_line() {
        let content = "single line";
        let index = LineIndex::new(content);
        assert_eq!(index.line_number_at(0), 1);
        assert_eq!(index.line_number_at(6), 1);
        assert_eq!(index.line_number_at(11), 1);
    }

    #[test]
    fn test_line_index_multiple_lines() {
        let content = "line 1\nline 2\nline 3";
        let index = LineIndex::new(content);

        // Line 1 (bytes 0-5)
        assert_eq!(index.line_number_at(0), 1);
        assert_eq!(index.line_number_at(5), 1);

        // Newline at byte 6
        assert_eq!(index.line_number_at(6), 1);

        // Line 2 (bytes 7-12)
        assert_eq!(index.line_number_at(7), 2);
        assert_eq!(index.line_number_at(12), 2);

        // Newline at byte 13
        assert_eq!(index.line_number_at(13), 2);

        // Line 3 (bytes 14-19)
        assert_eq!(index.line_number_at(14), 3);
        assert_eq!(index.line_number_at(19), 3);
    }

    #[test]
    fn test_line_index_spans_and_endings() {
        let content = "a\r\nbc\n\nd";
        let index = LineIndex::new(content);
        assert_eq!(index.newline_count(), 3);
        assert_eq!(index.line_span(1), 0..2);
        assert_eq!(index.line_end_byte(1), 1);
        assert_eq!(index.line_ending(1), LineEnding::CrLf);
        assert_eq!(index.line_span(2), 3..5);
        assert_eq!(index.line_ending(2), LineEnding::Lf);
        assert_eq!(index.line_span(3), 6..6);
        assert_eq!(index.line_span(4), 7..8);
        assert_eq!(index.line_ending(4), LineEnding::None);
        assert_eq!(index.line_end_byte(4), 8);
    }

    #[test]
    fn test_line_index_empty_lines() {
        let content = "line 1\n\nline 3";
        let index = LineIndex::new(content);

        assert_eq!(index.line_number_at(0), 1); // "l" in line 1
        assert_eq!(index.line_number_at(6), 1); // first newline
        assert_eq!(index.line_number_at(7), 2); // second newline (empty line)
        assert_eq!(index.line_number_at(8), 3); // "l" in line 3
    }
}
