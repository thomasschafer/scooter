use std::io::BufRead;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineEnding {
    /// No line ending (typically the last line of a file)
    None,
    /// Unix/Linux/macOS line ending (`\n`)
    Lf,
    /// Windows line ending (`\r\n`)
    CrLf,
}

impl LineEnding {
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::None => "",
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

/// An iterator that reads lines from a `BufRead` source while preserving line endings.
///
/// Unlike the standard library's `lines()` iterator which strips line endings,
/// this iterator returns tuples of `(content, line_ending)` where the line ending
/// is preserved as a separate string.
pub struct LinesSplitEndings<R> {
    reader: R,
    buffer: String,
}

impl<R: BufRead> LinesSplitEndings<R> {
    /// Creates a new `LinesSplitEndings` iterator from any type that implements `BufRead`.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: String::new(),
        }
    }
}

impl<R: BufRead> Iterator for LinesSplitEndings<R> {
    type Item = std::io::Result<(String, LineEnding)>;

    fn next(&mut self) -> Option<Self::Item> {
        self.buffer.clear();
        match self.reader.read_line(&mut self.buffer) {
            Ok(0) => None, // EOF
            Ok(_) => {
                let (content, ending) = split_line_ending(&self.buffer);
                Some(Ok((content.to_string(), ending)))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

/// Extension trait that adds the `lines_with_endings()` method to any `BufRead` implementation.
///
/// # Examples
///
/// ```
/// use std::io::Cursor;
/// use scooter_core::line_reader::BufReadExt;
///
/// let cursor = Cursor::new("hello\nworld\r\n");
///
/// for line_result in cursor.lines_with_endings() {
///     let (content, ending) = line_result?;
///     println!("Content: '{}', Ending: '{:?}'", content, ending);
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
pub trait BufReadExt: BufRead {
    /// Returns an iterator that yields lines with their endings preserved.
    ///
    /// Each item yielded by the iterator is a `Result<(String, LineEnding), io::Error>`
    /// where the first string is the line content and the second is the line ending type.
    fn lines_with_endings(self) -> LinesSplitEndings<Self>
    where
        Self: Sized,
    {
        LinesSplitEndings::new(self)
    }
}

impl<R: BufRead> BufReadExt for R {}

/// Splits a line into its content and line ending parts.
///
/// # Examples
///
/// ```
/// use scooter_core::line_reader::{split_line_ending, LineEnding};
///
/// assert_eq!(split_line_ending("hello\n"), ("hello", LineEnding::Lf));
/// assert_eq!(split_line_ending("hello\r\n"), ("hello", LineEnding::CrLf));
/// assert_eq!(split_line_ending("hello"), ("hello", LineEnding::None));
/// ```
#[inline]
pub fn split_line_ending(line: &str) -> (&str, LineEnding) {
    let len = line.len();
    if len == 0 {
        return (line, LineEnding::None);
    }

    let bytes = line.as_bytes();
    if bytes[len - 1] == b'\n' {
        if len >= 2 && bytes[len - 2] == b'\r' {
            (&line[..len - 2], LineEnding::CrLf)
        } else {
            (&line[..len - 1], LineEnding::Lf)
        }
    } else {
        (line, LineEnding::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_split_line_ending_empty() {
        assert_eq!(split_line_ending(""), ("", LineEnding::None));
    }

    #[test]
    fn test_split_line_ending_no_ending() {
        assert_eq!(
            split_line_ending("hello world"),
            ("hello world", LineEnding::None)
        );
    }

    #[test]
    fn test_split_line_ending_lf() {
        assert_eq!(split_line_ending("hello\n"), ("hello", LineEnding::Lf));
        assert_eq!(split_line_ending("\n"), ("", LineEnding::Lf));
    }

    #[test]
    fn test_split_line_ending_crlf() {
        assert_eq!(split_line_ending("hello\r\n"), ("hello", LineEnding::CrLf));
        assert_eq!(split_line_ending("\r\n"), ("", LineEnding::CrLf));
    }

    #[test]
    fn test_split_line_ending_unicode() {
        assert_eq!(
            split_line_ending("héllo 世界\n"),
            ("héllo 世界", LineEnding::Lf)
        );
        assert_eq!(
            split_line_ending("héllo 世界\r\n"),
            ("héllo 世界", LineEnding::CrLf)
        );
    }

    #[test]
    fn test_lines_split_endings_empty() {
        let cursor = Cursor::new("");
        let mut lines = LinesSplitEndings::new(cursor);
        assert!(lines.next().is_none());
    }

    #[test]
    fn test_lines_split_endings_single_line_no_ending() {
        let cursor = Cursor::new("hello");
        let mut lines = LinesSplitEndings::new(cursor);

        let result = lines.next().unwrap().unwrap();
        assert_eq!(result, ("hello".to_string(), LineEnding::None));

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_lines_split_endings_single_line_with_lf() {
        let cursor = Cursor::new("hello\n");
        let mut lines = LinesSplitEndings::new(cursor);

        let result = lines.next().unwrap().unwrap();
        assert_eq!(result, ("hello".to_string(), LineEnding::Lf));

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_lines_split_endings_multiple_lines_mixed() {
        let cursor = Cursor::new("line1\nline2\r\nline3\n\nline5");
        let mut lines = LinesSplitEndings::new(cursor);

        let result1 = lines.next().unwrap().unwrap();
        assert_eq!(result1, ("line1".to_string(), LineEnding::Lf));

        let result2 = lines.next().unwrap().unwrap();
        assert_eq!(result2, ("line2".to_string(), LineEnding::CrLf));

        let result3 = lines.next().unwrap().unwrap();
        assert_eq!(result3, ("line3".to_string(), LineEnding::Lf));

        let result4 = lines.next().unwrap().unwrap();
        assert_eq!(result4, ("".to_string(), LineEnding::Lf));

        let result5 = lines.next().unwrap().unwrap();
        assert_eq!(result5, ("line5".to_string(), LineEnding::None));

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_lines_split_endings_empty_lines() {
        let cursor = Cursor::new("\n\r\n\r\n");
        let mut lines = LinesSplitEndings::new(cursor);

        let result1 = lines.next().unwrap().unwrap();
        assert_eq!(result1, ("".to_string(), LineEnding::Lf));

        let result2 = lines.next().unwrap().unwrap();
        assert_eq!(result2, ("".to_string(), LineEnding::CrLf));

        let result3 = lines.next().unwrap().unwrap();
        assert_eq!(result3, ("".to_string(), LineEnding::CrLf));

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_buf_read_ext_trait() {
        let cursor = Cursor::new("hello\nworld\r\n");
        let mut lines = cursor.lines_with_endings();

        let result1 = lines.next().unwrap().unwrap();
        assert_eq!(result1, ("hello".to_string(), LineEnding::Lf));

        let result2 = lines.next().unwrap().unwrap();
        assert_eq!(result2, ("world".to_string(), LineEnding::CrLf));

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_large_line() {
        let content = "a".repeat(10000);
        let line = format!("{content}\n");
        let cursor = Cursor::new(line);
        let mut lines = LinesSplitEndings::new(cursor);

        let result = lines.next().unwrap().unwrap();
        assert_eq!(result, (content, LineEnding::Lf));

        assert!(lines.next().is_none());
    }

    #[test]
    fn test_unicode_content() {
        let content = "Hello 世界 🦀 Rust";
        let line = format!("{content}\r\n");
        let cursor = Cursor::new(line);
        let mut lines = LinesSplitEndings::new(cursor);

        let result = lines.next().unwrap().unwrap();
        assert_eq!(result, (content.to_string(), LineEnding::CrLf));

        assert!(lines.next().is_none());
    }
}
