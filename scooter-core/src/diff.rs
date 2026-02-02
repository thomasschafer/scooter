use similar::{Change, ChangeTag, TextDiff};

use crate::utils::group_by;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DiffColour {
    Red,
    Green,
    Black,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Diff {
    pub text: String,
    pub fg_colour: DiffColour,
    pub bg_colour: Option<DiffColour>,
}

pub fn line_diff<'a>(old_line: &'a str, new_line: &'a str) -> (Vec<Diff>, Vec<Diff>) {
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .timeout(std::time::Duration::from_millis(100))
        .diff_chars(old_line, new_line);

    let mut old_spans = Vec::new();
    let mut new_spans = Vec::new();

    for change_group in group_by(diff.iter_all_changes(), |c1, c2| c1.tag() == c2.tag()) {
        let first_change = change_group.first().unwrap(); // group_by should never return an empty group
        let text = change_group.iter().map(Change::value).collect();
        match first_change.tag() {
            ChangeTag::Delete => {
                old_spans.push(Diff {
                    text,
                    fg_colour: DiffColour::Black,
                    bg_colour: Some(DiffColour::Red),
                });
            }
            ChangeTag::Insert => {
                new_spans.push(Diff {
                    text,
                    fg_colour: DiffColour::Black,
                    bg_colour: Some(DiffColour::Green),
                });
            }
            ChangeTag::Equal => {
                old_spans.push(Diff {
                    text: text.clone(),
                    fg_colour: DiffColour::Red,
                    bg_colour: None,
                });
                new_spans.push(Diff {
                    text,
                    fg_colour: DiffColour::Green,
                    bg_colour: None,
                });
            }
        }
    }

    (old_spans, new_spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_lines() {
        let (old_actual, new_actual) = line_diff("hello", "hello");

        let old_expected = vec![Diff {
            text: "hello".to_owned(),
            fg_colour: DiffColour::Red,
            bg_colour: None,
        }];

        let new_expected = vec![Diff {
            text: "hello".to_owned(),
            fg_colour: DiffColour::Green,
            bg_colour: None,
        }];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_single_char_difference() {
        let (old_actual, new_actual) = line_diff("hello", "hallo");

        let old_expected = vec![
            Diff {
                text: "h".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
            Diff {
                text: "e".to_owned(),
                fg_colour: DiffColour::Black,
                bg_colour: Some(DiffColour::Red),
            },
            Diff {
                text: "llo".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "h".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
            Diff {
                text: "a".to_owned(),
                fg_colour: DiffColour::Black,
                bg_colour: Some(DiffColour::Green),
            },
            Diff {
                text: "llo".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
        ];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_completely_different_strings() {
        let (old_actual, new_actual) = line_diff("foo", "bar");

        let old_expected = vec![Diff {
            text: "foo".to_owned(),
            fg_colour: DiffColour::Black,
            bg_colour: Some(DiffColour::Red),
        }];

        let new_expected = vec![Diff {
            text: "bar".to_owned(),
            fg_colour: DiffColour::Black,
            bg_colour: Some(DiffColour::Green),
        }];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_empty_strings() {
        let (old_actual, new_actual) = line_diff("", "");

        let old_expected: Vec<Diff> = vec![];
        let new_expected: Vec<Diff> = vec![];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_addition_at_end() {
        let (old_actual, new_actual) = line_diff("hello", "hello!");

        let old_expected = vec![Diff {
            text: "hello".to_owned(),
            fg_colour: DiffColour::Red,
            bg_colour: None,
        }];

        let new_expected = vec![
            Diff {
                text: "hello".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
            Diff {
                text: "!".to_owned(),
                fg_colour: DiffColour::Black,
                bg_colour: Some(DiffColour::Green),
            },
        ];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_addition_at_start() {
        let (old_actual, new_actual) = line_diff("hello", "!hello");

        let old_expected = vec![Diff {
            text: "hello".to_owned(),
            fg_colour: DiffColour::Red,
            bg_colour: None,
        }];

        let new_expected = vec![
            Diff {
                text: "!".to_owned(),
                fg_colour: DiffColour::Black,
                bg_colour: Some(DiffColour::Green),
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
        ];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_newline_in_new_content() {
        let (old_actual, new_actual) = line_diff("hello", "hel\nlo");

        // Old content is unchanged but may be split into segments by the diff algorithm
        let old_text: String = old_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(old_text, "hello");

        // New content should contain the newline in the diff
        let new_text: String = new_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(new_text, "hel\nlo");
    }

    #[test]
    fn test_newline_in_old_content() {
        let (old_actual, new_actual) = line_diff("hel\nlo", "hello");

        // Old content contains the newline
        let old_text: String = old_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(old_text, "hel\nlo");

        // New content has it removed
        let new_text: String = new_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(new_text, "hello");
    }

    #[test]
    fn test_replacement_with_only_newlines() {
        let (old_actual, new_actual) = line_diff("abc", "\n\n");

        let old_text: String = old_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(old_text, "abc");

        let new_text: String = new_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(new_text, "\n\n");
    }

    #[test]
    fn test_unicode_multibyte_chars() {
        let (old_actual, new_actual) = line_diff("héllo", "hëllo");

        let old_text: String = old_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(old_text, "héllo");

        let new_text: String = new_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(new_text, "hëllo");
    }

    #[test]
    fn test_unicode_cjk_characters() {
        let (old_actual, new_actual) = line_diff("世界", "世間");

        let old_text: String = old_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(old_text, "世界");

        let new_text: String = new_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(new_text, "世間");
    }

    #[test]
    fn test_empty_to_nonempty() {
        let (old_actual, new_actual) = line_diff("", "hello");

        assert!(old_actual.is_empty());

        assert_eq!(new_actual.len(), 1);
        assert_eq!(new_actual[0].text, "hello");
        assert_eq!(new_actual[0].bg_colour, Some(DiffColour::Green));
    }

    #[test]
    fn test_nonempty_to_empty() {
        let (old_actual, new_actual) = line_diff("hello", "");

        assert_eq!(old_actual.len(), 1);
        assert_eq!(old_actual[0].text, "hello");
        assert_eq!(old_actual[0].bg_colour, Some(DiffColour::Red));

        assert!(new_actual.is_empty());
    }

    #[test]
    fn test_crlf_in_content() {
        let (old_actual, new_actual) = line_diff("hello", "hel\r\nlo");

        let old_text: String = old_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(old_text, "hello");

        let new_text: String = new_actual.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(new_text, "hel\r\nlo");
    }
}
