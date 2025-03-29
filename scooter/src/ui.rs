use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Clear, List, ListItem, Paragraph},
    Frame,
};
use similar::{Change, ChangeTag, TextDiff};
use std::{
    cmp::min,
    iter,
    path::{Path, PathBuf},
};

use crate::{
    app::{
        App, AppError, FieldName, ReplaceResult, ReplaceState, Screen, SearchField, SearchResult,
        SearchState, NUM_SEARCH_FIELDS,
    },
    utils::{group_by, relative_path_from},
};

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

fn render_search_view(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let areas: [Rect; NUM_SEARCH_FIELDS] =
        Layout::vertical(iter::repeat(Constraint::Length(3)).take(app.search_fields.fields.len()))
            .flex(Flex::Center)
            .areas(area);

    app.search_fields
        .fields
        .iter()
        .zip(areas)
        .enumerate()
        .for_each(|(idx, (SearchField { name, field }, field_area))| {
            field.read().render(
                frame,
                field_area,
                name.title().to_owned(),
                idx == app.search_fields.highlighted,
            )
        });

    if !app.show_error_popup() {
        if let Some(cursor_pos) = app.search_fields.highlighted_field().read().cursor_pos() {
            let highlighted_area = areas[app.search_fields.highlighted];

            frame.set_cursor(
                highlighted_area.x + cursor_pos as u16 + 1,
                highlighted_area.y + 1,
            )
        }
    }
}

fn strip_control_chars(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' => String::from("  "),
            '\n' => String::from(" "),
            c if c.is_control() => String::from("�"),
            c => String::from(c),
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Diff {
    pub text: String,
    pub fg_colour: Color,
    pub bg_colour: Color,
}

fn diff_to_line(diff: Vec<&Diff>) -> Line<'static> {
    let diff_iter = diff.into_iter().map(|d| {
        let style = Style::new().fg(d.fg_colour).bg(d.bg_colour);
        Span::styled(strip_control_chars(&d.text), style)
    });
    Line::from_iter(diff_iter)
}

pub fn line_diff<'a>(old_line: &'a str, new_line: &'a str) -> (Vec<Diff>, Vec<Diff>) {
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .timeout(std::time::Duration::from_millis(100))
        .diff_chars(old_line, new_line);

    let mut old_spans = vec![Diff {
        text: "- ".to_owned(),
        fg_colour: Color::Red,
        bg_colour: Color::Reset,
    }];
    let mut new_spans = vec![Diff {
        text: "+ ".to_owned(),
        fg_colour: Color::Green,
        bg_colour: Color::Reset,
    }];

    for change_group in group_by(diff.iter_all_changes(), |c1, c2| c1.tag() == c2.tag()) {
        let first_change = change_group.first().unwrap(); // group_by should never return an empty group
        let text = change_group.iter().map(Change::value).collect();
        match first_change.tag() {
            ChangeTag::Delete => {
                old_spans.push(Diff {
                    text,
                    fg_colour: Color::Black,
                    bg_colour: Color::Red,
                });
            }
            ChangeTag::Insert => {
                new_spans.push(Diff {
                    text,
                    fg_colour: Color::Black,
                    bg_colour: Color::Green,
                });
            }
            ChangeTag::Equal => {
                old_spans.push(Diff {
                    text: text.clone(),
                    fg_colour: Color::Red,
                    bg_colour: Color::Reset,
                });
                new_spans.push(Diff {
                    text,
                    fg_colour: Color::Green,
                    bg_colour: Color::Reset,
                });
            }
        };
    }

    (old_spans, new_spans)
}

fn render_confirmation_view(
    frame: &mut Frame<'_>,
    is_complete: bool,
    search_state: &mut SearchState,
    base_path: PathBuf,
    area: Rect,
) {
    let [num_results_area, list_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)])
            .flex(Flex::Start)
            .areas(area);

    let list_area_height = list_area.height as usize;
    let item_height = 4; // TODO: find a better way of doing this
    let num_results = search_state.results.len();

    frame.render_widget(
        Span::raw(format!(
            "Results: {} {}",
            num_results,
            if is_complete {
                "[Search complete]"
            } else {
                "[Still searching...]"
            }
        )),
        num_results_area,
    );

    let num_to_render = list_area_height / item_height;
    if search_state.selected() < search_state.view_offset + 1 {
        search_state.view_offset = search_state.selected().saturating_sub(1);
    } else if search_state.selected() > (search_state.view_offset + num_to_render).saturating_sub(2)
        || search_state.view_offset + num_to_render > num_results
    {
        search_state.view_offset = min(
            (search_state.selected() + 2).saturating_sub(num_to_render),
            num_results.saturating_sub(num_to_render),
        );
    }

    let search_results = search_state
        .results
        .iter()
        .enumerate()
        .skip(search_state.view_offset)
        .take(num_to_render)
        .flat_map(|(idx, result)| {
            render_search_result(
                idx,
                search_state.selected(),
                result,
                &base_path,
                list_area.width,
            )
        });

    frame.render_widget(List::new(search_results), list_area);
}

fn render_search_result<'a>(
    idx: usize,
    selected: usize,
    result: &SearchResult,
    base_path: &Path,
    list_area_width: u16,
) -> [ListItem<'a>; 4] {
    let (old_line, new_line) = line_diff(&result.line, &result.replacement);
    let old_line = old_line
        .iter()
        .take(list_area_width as usize)
        .collect::<Vec<_>>();
    let new_line = new_line
        .iter()
        .take(list_area_width as usize)
        .collect::<Vec<_>>();

    let file_path_style = if idx == selected {
        Style::new().bg(if result.included {
            Color::Blue
        } else {
            Color::Red
        })
    } else {
        Style::new()
    };
    let right_content = format!(" ({})", idx + 1);
    let right_content_len = right_content.len() as u16;
    let left_content = format!(
        "[{}] {}:{}",
        if result.included { 'x' } else { ' ' },
        relative_path_from(base_path, &result.path),
        result.line_number,
    );
    let left_content_trimmed = left_content
        .chars()
        .take(list_area_width.saturating_sub(right_content_len) as usize)
        .collect::<String>();
    let left_content_trimmed_len = left_content_trimmed.len() as u16;
    let spacers = " ".repeat(
        list_area_width.saturating_sub(left_content_trimmed_len + right_content_len) as usize,
    );

    let file_path = Line::from(vec![
        Span::raw(left_content_trimmed),
        Span::raw(spacers),
        Span::raw(right_content),
    ])
    .style(file_path_style);

    [
        ListItem::new(file_path),
        ListItem::new(diff_to_line(old_line)),
        ListItem::new(diff_to_line(new_line)),
        ListItem::new(""),
    ]
}

fn render_results_view(frame: &mut Frame<'_>, replace_state: &ReplaceState, area: Rect) {
    if replace_state.errors.is_empty() {
        render_results_success(area, replace_state, frame);
    } else {
        render_results_errors(area, replace_state, frame);
    }
}

const ERROR_ITEM_HEIGHT: u16 = 3;
const NUM_TALLIES: usize = 3;

fn render_results_success(area: Rect, replace_state: &ReplaceState, frame: &mut Frame<'_>) {
    let [_, success_title_area, results_area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(ERROR_ITEM_HEIGHT * NUM_TALLIES as u16), // TODO: find a better way of doing this
        Constraint::Fill(1),
    ])
    .flex(Flex::Start)
    .areas(area);

    render_results_tallies(results_area, frame, replace_state);

    let text = "Success!";
    let area = center(
        success_title_area,
        Constraint::Length(text.len() as u16), // TODO: find a better way of doing this
        Constraint::Length(1),
    );
    frame.render_widget(Text::raw(text), area);
}

fn render_results_errors(area: Rect, replace_state: &ReplaceState, frame: &mut Frame<'_>) {
    let [results_area, list_title_area, list_area] = Layout::vertical([
        Constraint::Length(ERROR_ITEM_HEIGHT * NUM_TALLIES as u16), // TODO: find a better way of doing this
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .flex(Flex::Start)
    .areas(area);

    let errors = replace_state
        .errors
        .iter()
        .map(|res| {
            error_result(
                res,
                match &res.replace_result {
                    Some(ReplaceResult::Error(error)) => error,
                    None => panic!("Found error result with no error message"),
                    Some(ReplaceResult::Success) => {
                        panic!("Found successful result in errors: {:?}", res)
                    }
                },
            )
        })
        .skip(replace_state.replacement_errors_pos)
        .take(list_area.height as usize / 3 + 1); // TODO: don't hardcode height

    render_results_tallies(results_area, frame, replace_state);

    frame.render_widget(Text::raw("Errors:"), list_title_area);
    frame.render_widget(List::new(errors.flatten()), list_area);
}

fn render_results_tallies(results_area: Rect, frame: &mut Frame<'_>, replace_state: &ReplaceState) {
    let [success_area, ignored_area, errors_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .flex(Flex::Start)
    .areas(results_area);
    let widgets: [_; NUM_TALLIES] = [
        (
            "Successful replacements:",
            replace_state.num_successes,
            success_area,
        ),
        ("Ignored:", replace_state.num_ignored, ignored_area),
        ("Errors:", replace_state.errors.len(), errors_area),
    ];
    let widgets = widgets.into_iter().map(|(title, num, area)| {
        (
            Paragraph::new(num.to_string())
                .block(Block::bordered().border_style(Style::new()).title(title)),
            area,
        )
    });
    widgets.for_each(|(widget, area)| {
        frame.render_widget(widget, area);
    });
}

fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

fn render_loading_view(text: String) -> impl Fn(&mut Frame<'_>, &App, Rect) {
    move |frame: &mut Frame<'_>, _app: &App, area: Rect| {
        let [area] = Layout::vertical([Constraint::Length(4)])
            .flex(Flex::Center)
            .areas(area);

        let text = Paragraph::new(Line::from(Span::raw(&text)))
            .block(Block::default())
            .alignment(Alignment::Center);

        frame.render_widget(text, area);
    }
}

fn error_result(result: &SearchResult, error: &str) -> [ratatui::widgets::ListItem<'static>; 3] {
    [
        ("".to_owned(), Style::default()),
        (
            format!(
                "{}:{}",
                result
                    .path
                    .clone()
                    .into_os_string()
                    .into_string()
                    .expect("Failed to display path"),
                result.line_number
            ),
            Style::default(),
        ),
        (error.to_owned(), Style::default().fg(Color::Red)),
    ]
    .map(|(s, style)| ListItem::new(Text::styled(s, style)))
}

pub fn render(app: &mut App, frame: &mut Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.size());

    let title_block = Block::default().style(Style::default());
    let title = Paragraph::new(Text::styled("Scooter", Style::default()))
        .block(title_block)
        .alignment(Alignment::Center);
    frame.render_widget(title, chunks[0]);

    let [content_area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(chunks[1]);

    render_key_hints(app, frame, chunks[2]);

    let base_path = app.directory.clone();
    match &mut app.current_screen {
        Screen::SearchFields => render_search_view(frame, app, content_area),
        Screen::SearchProgressing(ref mut s) => {
            render_confirmation_view(frame, false, &mut s.search_state, base_path, content_area);
        }
        Screen::SearchComplete(ref mut s) => {
            render_confirmation_view(frame, true, s, base_path, content_area);
        }
        Screen::PerformingReplacement(_) => {
            render_loading_view("Performing replacement...".to_owned())(frame, app, content_area);
        }
        Screen::Results(ref replace_state) => {
            render_results_view(frame, replace_state, content_area);
        }
    };

    if app.show_error_popup() {
        render_error_popup(app, frame, content_area);
    }
}

fn render_key_hints(app: &App, frame: &mut Frame<'_>, chunk: Rect) {
    let current_keys = match app.current_screen {
        Screen::SearchFields => {
            vec!["<enter> search", "<tab> focus next", "<S-tab> focus prev"]
        }
        Screen::SearchProgressing(_) | Screen::SearchComplete(_) => {
            let mut keys = if let Screen::SearchComplete(_) = app.current_screen {
                vec!["<enter> replace selected"]
            } else {
                vec![]
            };
            keys.append(&mut vec![
                "<space> toggle",
                "<a> toggle all",
                "<o> open",
                "<C-o> back",
            ]);
            keys
        }
        Screen::PerformingReplacement(_) => vec![],
        Screen::Results(ref replace_state) => {
            if !replace_state.errors.is_empty() {
                vec!["<j> down", "<k> up"]
            } else {
                vec![]
            }
        }
    };

    let additional_keys = ["<C-r> reset", "<esc> quit"];

    let all_keys = current_keys
        .iter()
        .chain(additional_keys.iter())
        .join(" / ");
    let keys_hint = Span::styled(all_keys, Color::default());

    let footer = Paragraph::new(Line::from(keys_hint))
        .block(Block::default())
        .alignment(Alignment::Center);
    frame.render_widget(footer, chunk);
}

fn render_error_popup(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let error_lines: Vec<Line<'_>> = app
        .errors()
        .into_iter()
        .flat_map(|AppError { name, long, .. }| {
            let name_line = Line::from(vec![Span::styled(name, Style::default().bold())]);

            let error_lines: Vec<Line<'_>> = long
                .lines()
                .map(|line| {
                    Line::from(vec![Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::Red),
                    )])
                })
                .collect();

            std::iter::once(name_line)
                .chain(error_lines)
                .chain(std::iter::once(Line::from("")))
                .collect::<Vec<_>>()
        })
        .collect();

    let content_height = error_lines.len() as u16 + 1;

    let popup_area = center(
        area,
        Constraint::Percentage(80),
        Constraint::Length(content_height),
    );

    let popup = Paragraph::new(error_lines).block(
        Block::bordered()
            .title("Errors")
            .title_alignment(Alignment::Center),
    );
    frame.render_widget(Clear, popup_area);
    frame.render_widget(popup, popup_area);
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    #[test]
    fn test_identical_lines() {
        let (old_actual, new_actual) = line_diff("hello", "hello");

        let old_expected = vec![
            Diff {
                text: "- ".to_owned(),
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
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
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "h".to_owned(),
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "e".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Color::Red,
            },
            Diff {
                text: "llo".to_owned(),
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "h".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "a".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Color::Green,
            },
            Diff {
                text: "llo".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
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
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "foo".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Color::Red,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "bar".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Color::Green,
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
            fg_colour: Color::Red,
            bg_colour: Color::Reset,
        }];

        let new_expected = vec![Diff {
            text: "+ ".to_owned(),
            fg_colour: Color::Green,
            bg_colour: Color::Reset,
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
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "!".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Color::Green,
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
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Red,
                bg_colour: Color::Reset,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
            Diff {
                text: "!".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Color::Green,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Green,
                bg_colour: Color::Reset,
            },
        ];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_sanitize_normal_text() {
        assert_eq!(strip_control_chars("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_tabs() {
        assert_eq!(strip_control_chars("hello\tworld"), "hello  world");
        assert_eq!(strip_control_chars("\t\t"), "    ");
    }

    #[test]
    fn test_sanitize_newlines() {
        assert_eq!(strip_control_chars("hello\nworld"), "hello world");
        assert_eq!(strip_control_chars("\n\n"), "  ");
    }

    #[test]
    fn test_sanitize_control_chars() {
        assert_eq!(strip_control_chars("hello\u{4}world"), "hello�world");
        assert_eq!(strip_control_chars("test\u{7}"), "test�");
        assert_eq!(strip_control_chars("\u{1b}[0m"), "�[0m");
    }

    #[test]
    fn test_sanitize_unicode() {
        assert_eq!(strip_control_chars("héllo→世界"), "héllo→世界");
    }

    #[test]
    fn test_sanitize_empty_string() {
        assert_eq!(strip_control_chars(""), "");
    }

    #[test]
    fn test_sanitize_only_control_chars() {
        assert_eq!(strip_control_chars("\u{1}\u{2}\u{3}\u{4}"), "����");
    }
}
