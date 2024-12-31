use itertools::Itertools;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Clear, List, ListItem, Paragraph},
    Frame,
};
use similar::{Change, ChangeTag, TextDiff};
use std::{cmp::min, iter};

use crate::{
    app::{
        App, FieldName, ReplaceState, Screen, SearchField, SearchInProgressState, NUM_SEARCH_FIELDS,
    },
    event::{ReplaceResult, SearchResult},
    utils::{first_chars, group_by},
};

impl FieldName {
    pub(crate) fn title(&self) -> &str {
        match self {
            FieldName::Search => "Search text",
            FieldName::Replace => "Replace text",
            FieldName::FixedStrings => "Fixed strings",
            FieldName::PathPattern => "Path pattern (regex)",
        }
    }
}

fn render_search_view(frame: &mut Frame<'_>, app: &App, rect: Rect) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(rect);
    let areas: [Rect; NUM_SEARCH_FIELDS] =
        Layout::vertical(iter::repeat(Constraint::Length(4)).take(app.search_fields.fields.len()))
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

    if app.search_fields.show_error_popup {
        let error_lines: Vec<Line<'_>> = app
            .search_fields
            .errors()
            .iter()
            .flat_map(|(name, error)| {
                let name_line = Line::from(vec![Span::styled(*name, Style::default().bold())]);

                let error_lines: Vec<Line<'_>> = error
                    .long
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
    } else if let Some(cursor_idx) = app.search_fields.highlighted_field().read().cursor_idx() {
        let highlighted_area = areas[app.search_fields.highlighted];

        frame.set_cursor(
            highlighted_area.x + cursor_idx as u16 + 1,
            highlighted_area.y + 1,
        )
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Diff {
    pub text: String,
    pub fg_colour: Color,
    pub bg_colour: Color,
}

fn diff_to_line(diff: Vec<Diff>) -> Line<'static> {
    let diff_iter = diff.into_iter().map(|d| {
        let style = Style::new().fg(d.fg_colour).bg(d.bg_colour);
        Span::styled(d.text, style)
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

fn render_confirmation_view(frame: &mut Frame<'_>, app: &App, rect: Rect) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(rect);
    let [num_results_area, list_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)])
            .flex(Flex::Start)
            .areas(area);

    let (is_complete, search_results) = match &app.current_screen {
        Screen::SearchProgressing(SearchInProgressState { search_state, .. }) => {
            (false, search_state)
        }
        Screen::SearchComplete(search_state) => (true, search_state),
        // prevent race condition when state is being reset
        _ => return,
    };

    let list_area_height = list_area.height as usize;
    let item_height = 4; // TODO: find a better way of doing this
    let midpoint = list_area_height / (2 * item_height);
    let num_results = search_results.results.len();

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

    let results_iter = search_results
        .results
        .iter()
        .enumerate()
        .skip(min(
            search_results.selected.saturating_sub(midpoint),
            num_results.saturating_sub(list_area_height / item_height),
        ))
        .take(list_area_height / item_height + 1); // We shouldn't need the +1, but let's keep it in to ensure we have buffer when rendering

    let search_results = results_iter.flat_map(|(idx, result)| {
        let width = list_area.width;
        let before = first_chars(&result.line, width as usize);
        let after = first_chars(&result.replacement, width as usize);
        let (old_line, new_line) = line_diff(before, after);

        let file_path_style = if search_results.selected == idx {
            Style::new().bg(if result.included {
                Color::Blue
            } else {
                Color::Red
            })
        } else {
            Style::new()
        };
        let right_content = format!(" ({})", idx);
        let right_content_len = right_content.len() as u16;
        let left_content = format!(
            "[{}] {}:{}",
            if result.included { 'x' } else { ' ' },
            app.relative_path(&result.path),
            result.line_number,
        );
        let left_content_trimmed = left_content
            .chars()
            .take(list_area.width.saturating_sub(right_content_len) as usize)
            .collect::<String>();
        let left_content_trimmed_len = left_content_trimmed.len() as u16;
        let spacers = " ".repeat(
            list_area
                .width
                .saturating_sub(left_content_trimmed_len + right_content_len) as usize,
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
    });

    frame.render_widget(List::new(search_results), list_area);
}

fn render_results_view(replace_state: &ReplaceState) -> impl Fn(&mut Frame<'_>, &App, Rect) + '_ {
    move |frame: &mut Frame<'_>, _app: &App, rect: Rect| {
        let [area] = Layout::horizontal([Constraint::Percentage(80)])
            .flex(Flex::Center)
            .areas(rect);

        if replace_state.errors.is_empty() {
            render_results_success(area, replace_state, frame);
        } else {
            render_results_errors(area, replace_state, frame);
        }
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
    move |frame: &mut Frame<'_>, _app: &App, rect: Rect| {
        let [area] = Layout::vertical([Constraint::Length(4)])
            .flex(Flex::Center)
            .areas(rect);

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

type RenderFn<'a> = Box<dyn Fn(&mut Frame<'_>, &'a App, Rect) + 'a>;

pub fn render(app: &App, frame: &mut Frame<'_>) {
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

    let render_fn: RenderFn<'_> = match &app.current_screen {
        Screen::SearchFields => Box::new(render_search_view),
        Screen::SearchProgressing(_) | Screen::SearchComplete(_) => {
            Box::new(render_confirmation_view)
        }
        Screen::PerformingReplacement(_) => {
            Box::new(render_loading_view("Performing replacement...".to_owned()))
        }
        Screen::Results(ref replace_state) => Box::new(render_results_view(replace_state)),
    };
    render_fn(frame, app, chunks[1]);

    let current_keys = match app.current_screen {
        Screen::SearchFields => {
            vec!["<enter> search", "<tab> focus next", "<S-tab> focus prev"]
        }
        Screen::SearchProgressing(_) | Screen::SearchComplete(_) => {
            let mut keys = if let Screen::SearchComplete(_) = app.current_screen {
                vec!["<enter> replace"]
            } else {
                vec![]
            };
            keys.append(&mut vec![
                "<space> toggle",
                "<a> toggle all",
                "<j> down",
                "<k> up",
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
    frame.render_widget(footer, chunks[2]);
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
}
