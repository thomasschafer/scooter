use similar::{Change, ChangeTag, TextDiff};

use crate::utils::group_by;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DiffColour {
    Red,
    Green,
    Black,
}

impl DiffColour {
    pub fn to_str(&self) -> &str {
        match self {
            DiffColour::Red => "red",
            DiffColour::Green => "green",
            DiffColour::Black => "black",
        }
    }
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

    let mut old_spans = vec![Diff {
        text: "- ".to_owned(),
        fg_colour: DiffColour::Red,
        bg_colour: None,
    }];
    let mut new_spans = vec![Diff {
        text: "+ ".to_owned(),
        fg_colour: DiffColour::Green,
        bg_colour: None,
    }];

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

        let old_expected = vec![
            Diff {
                text: "- ".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
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
    fn test_single_char_difference() {
        let (old_actual, new_actual) = line_diff("hello", "hallo");

        let old_expected = vec![
            Diff {
                text: "- ".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
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
                text: "+ ".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
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

        let old_expected = vec![
            Diff {
                text: "- ".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
            Diff {
                text: "foo".to_owned(),
                fg_colour: DiffColour::Black,
                bg_colour: Some(DiffColour::Red),
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
            Diff {
                text: "bar".to_owned(),
                fg_colour: DiffColour::Black,
                bg_colour: Some(DiffColour::Green),
            },
        ];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_empty_strings() {
        let (old_actual, new_actual) = line_diff("", "");

        let old_expected = vec![Diff {
            text: "- ".to_owned(),
            fg_colour: DiffColour::Red,
            bg_colour: None,
        }];

        let new_expected = vec![Diff {
            text: "+ ".to_owned(),
            fg_colour: DiffColour::Green,
            bg_colour: None,
        }];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_addition_at_end() {
        let (old_actual, new_actual) = line_diff("hello", "hello!");

        let old_expected = vec![
            Diff {
                text: "- ".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
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

        let old_expected = vec![
            Diff {
                text: "- ".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: DiffColour::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: DiffColour::Green,
                bg_colour: None,
            },
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
}
