use anyhow::{bail, Context};
use itertools::Itertools;
use lru::LruCache;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Position, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Cell, Clear, List, ListItem, Paragraph, Row, Table},
    Frame,
};
use similar::{Change, ChangeTag, TextDiff};
use std::{
    cmp::min,
    fs, iter,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use syntect::{
    highlighting::{FontStyle, Style as SyntectStyle, Theme},
    parsing::SyntaxSet,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    app::{
        App, AppError, AppEvent, Event, FieldName, Popup, ReplaceResult, ReplaceState, Screen,
        SearchField, SearchResult, SearchState, NUM_SEARCH_FIELDS,
    },
    fields::Field,
    utils::{
        group_by, largest_range_centered_on, last_n_chars, read_lines_range,
        read_lines_range_highlighted, relative_path_from, strip_control_chars, HighlightedLine,
    },
};

use super::colour::to_ratatui_colour;

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

impl Field {
    fn create_title_spans<'a>(&self, title: &'a str, highlighted: bool) -> Vec<Span<'a>> {
        let title_style = Style::new().fg(if highlighted {
            Color::Green
        } else {
            Color::Reset
        });

        let mut spans = vec![Span::styled(title, title_style)];
        if let Some(error) = self.error() {
            spans.push(Span::styled(
                format!(" (Error: {})", error.short),
                Style::new().fg(Color::Red),
            ));
        }
        spans
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, title: &str, highlighted: bool) {
        let mut block = Block::bordered();
        if highlighted {
            block = block.border_style(Style::new().green());
        }

        let title_spans = self.create_title_spans(title, highlighted);

        match self {
            Field::Text(f) => {
                block = block.title(Line::from(title_spans));
                frame.render_widget(Paragraph::new(f.text()).block(block), area);
            }
            Field::Checkbox(f) => {
                let inner_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(5), Constraint::Min(0)])
                    .split(area);

                frame.render_widget(
                    Paragraph::new(if f.checked { " X " } else { "" }).block(block),
                    inner_chunks[0],
                );

                let mut spans = vec![Span::raw(" ")];
                spans.extend(title_spans);

                let checkbox_text = vec![Line::from(Span::raw("")), Line::from(spans)];

                frame.render_widget(Paragraph::new(checkbox_text), inner_chunks[1]);
            }
        }
    }
}

fn render_search_view(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let area = default_width(area);
    let areas: [Rect; NUM_SEARCH_FIELDS] = Layout::vertical(iter::repeat_n(
        Constraint::Length(3),
        app.search_fields.fields.len(),
    ))
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
                name.title(),
                idx == app.search_fields.highlighted && !app.show_popup(),
            );
        });

    if !app.show_popup() {
        if let Some(cursor_pos) = app.search_fields.highlighted_field().read().cursor_pos() {
            let highlighted_area = areas[app.search_fields.highlighted];

            frame.set_cursor_position(Position {
                x: highlighted_area.x + u16::try_from(cursor_pos).unwrap_or(0) + 1,
                y: highlighted_area.y + 1,
            });
        }
    }
}

fn default_width(area: Rect) -> Rect {
    width(area, 80)
}

fn width(area: Rect, percentage: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Percentage(percentage)])
        .flex(Flex::Center)
        .areas(area);
    area
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Diff {
    pub text: String,
    pub fg_colour: Color,
    pub bg_colour: Option<Color>,
}

fn diff_to_line(diff: Vec<&Diff>) -> Line<'static> {
    let diff_iter = diff.into_iter().map(|d| {
        let mut style = Style::new().fg(d.fg_colour);
        if let Some(bg) = d.bg_colour {
            style = style.bg(bg);
        }
        Span::styled(strip_control_chars(&d.text), style)
    });
    diff_iter.collect()
}

pub fn line_diff<'a>(old_line: &'a str, new_line: &'a str) -> (Vec<Diff>, Vec<Diff>) {
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .timeout(std::time::Duration::from_millis(100))
        .diff_chars(old_line, new_line);

    let mut old_spans = vec![Diff {
        text: "- ".to_owned(),
        fg_colour: Color::Red,
        bg_colour: None,
    }];
    let mut new_spans = vec![Diff {
        text: "+ ".to_owned(),
        fg_colour: Color::Green,
        bg_colour: None,
    }];

    for change_group in group_by(diff.iter_all_changes(), |c1, c2| c1.tag() == c2.tag()) {
        let first_change = change_group.first().unwrap(); // group_by should never return an empty group
        let text = change_group.iter().map(Change::value).collect();
        match first_change.tag() {
            ChangeTag::Delete => {
                old_spans.push(Diff {
                    text,
                    fg_colour: Color::Black,
                    bg_colour: Some(Color::Red),
                });
            }
            ChangeTag::Insert => {
                new_spans.push(Diff {
                    text,
                    fg_colour: Color::Black,
                    bg_colour: Some(Color::Green),
                });
            }
            ChangeTag::Equal => {
                old_spans.push(Diff {
                    text: text.clone(),
                    fg_colour: Color::Red,
                    bg_colour: None,
                });
                new_spans.push(Diff {
                    text,
                    fg_colour: Color::Green,
                    bg_colour: None,
                });
            }
        }
    }

    (old_spans, new_spans)
}

fn display_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let milliseconds = duration.subsec_millis();
    format!("{seconds}.{milliseconds:03}s")
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn render_confirmation_view(
    frame: &mut Frame<'_>,
    is_complete: bool,
    search_state: &mut SearchState,
    time_taken: Duration,
    base_path: &Path,
    area: Rect,
    theme: Option<&Theme>,
    true_colour: bool,
    event_sender: UnboundedSender<Event>,
) {
    let split_view = area.width >= 110;
    let area = width(area, 90);

    let [num_results_area, results_area, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .flex(Flex::Start)
    .areas(area);

    let num_results = search_state.results.len();

    render_num_results(
        frame,
        num_results_area,
        num_results,
        is_complete,
        time_taken,
    );

    let num_to_render = if split_view {
        results_area.height as usize
    } else {
        5
    };

    search_state.num_displayed = Some(num_to_render);

    if search_state.primary_selected_pos() < search_state.view_offset + 1 {
        search_state.view_offset = search_state.primary_selected_pos().saturating_sub(1);
    } else if search_state.primary_selected_pos()
        > (search_state.view_offset + num_to_render).saturating_sub(2)
        || search_state.view_offset + num_to_render > num_results
    {
        search_state.view_offset = min(
            (search_state.primary_selected_pos() + 2).saturating_sub(num_to_render),
            num_results.saturating_sub(num_to_render),
        );
    }

    let (list_area, preview_area) = if split_view {
        let [list_area, _, preview_area] = Layout::horizontal([
            Constraint::Fill(2),
            Constraint::Length(1),
            Constraint::Fill(3),
        ])
        .areas(results_area);
        (list_area, preview_area)
    } else {
        let [list_area, _, preview_area] = Layout::vertical([
            #[allow(clippy::cast_possible_truncation)]
            Constraint::Length(num_to_render as u16),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(results_area);
        (list_area, preview_area)
    };

    let search_results =
        build_search_results(search_state, base_path, list_area.width, num_to_render);
    let search_results_list = search_results
        .iter()
        .map(|SearchResultLines { file_path, .. }| ListItem::new(file_path.clone()));
    frame.render_widget(List::new(search_results_list), list_area);

    if !search_results.is_empty() {
        let selected = search_results
            .iter()
            .find(|s| s.is_primary_selected)
            .expect("Selected item should be in view");
        let lines_to_show = preview_area.height as usize;

        match build_preview_list(lines_to_show, selected, theme, true_colour, event_sender) {
            Ok(preview) => {
                frame.render_widget(preview, preview_area);
            }
            Err(e) => {
                frame.render_widget(
                    Span::raw(format!("Error generating preview: \n{e}")).fg(Color::Red),
                    preview_area,
                );
            }
        };
    }
}

fn render_num_results(
    frame: &mut Frame<'_>,
    area: Rect,
    num_results: usize,
    is_complete: bool,
    time_taken: Duration,
) {
    let left_content_1 = format!("Results: {num_results}");
    let left_content_2 = if is_complete {
        " [Search complete]"
    } else {
        " [Still searching...]"
    };
    let right_content = format!(" [Time taken: {}]", display_duration(time_taken));
    let spacers = " ".repeat(
        (area.width as usize)
            .saturating_sub(left_content_1.len() + left_content_2.len() + right_content.len()),
    );

    let accessory_colour = if is_complete {
        Color::Green
    } else {
        Color::Blue
    };

    frame.render_widget(
        Line::from(vec![
            Span::raw(left_content_1),
            Span::raw(left_content_2).fg(accessory_colour),
            Span::raw(spacers),
            Span::raw(right_content).fg(accessory_colour),
        ]),
        area,
    );
}

fn build_search_results<'a>(
    search_state: &'a mut SearchState,
    base_path: &Path,
    width: u16,
    num_to_render: usize,
) -> Vec<SearchResultLines<'a>> {
    search_state
        .results
        .iter()
        .enumerate()
        .skip(search_state.view_offset)
        .take(num_to_render)
        .map(|(idx, result)| {
            search_result(
                idx,
                search_state.is_selected(idx),
                search_state.is_primary_selected(idx),
                result,
                base_path,
                width,
            )
        })
        .collect()
}

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

fn convert_syntect_to_ratatui_style(syntect_style: &SyntectStyle, true_colour: bool) -> Style {
    let mut ratatui_style = Style::default()
        .fg(to_ratatui_colour(syntect_style.foreground, true_colour))
        .bg(to_ratatui_colour(syntect_style.background, true_colour));

    if syntect_style.font_style.contains(FontStyle::BOLD) {
        ratatui_style = ratatui_style.bold();
    }
    if syntect_style.font_style.contains(FontStyle::ITALIC) {
        ratatui_style = ratatui_style.italic();
    }
    if syntect_style.font_style.contains(FontStyle::UNDERLINE) {
        ratatui_style = ratatui_style.underlined();
    }
    ratatui_style
}

fn regions_to_line<'a>(line: &[(Option<SyntectStyle>, String)], true_colour: bool) -> ListItem<'a> {
    let prefix = "  ";
    ListItem::new(
        iter::once(Span::raw(prefix))
            .chain(line.iter().map(|(style, s)| {
                Span::styled(
                    strip_control_chars(s),
                    match style {
                        Some(style) => convert_syntect_to_ratatui_style(style, true_colour),
                        None => Style::default(),
                    },
                )
            }))
            .collect::<Line<'_>>(),
    )
}

fn to_line_plain<'a>(line: &str) -> ListItem<'a> {
    let prefix = "  ";
    ListItem::new(format!("{prefix}{}", strip_control_chars(line)))
}

type HighlightedLinesCache = Mutex<LruCache<PathBuf, Vec<(usize, HighlightedLine)>>>;

static HIGHLIGHTED_LINES_CACHE: OnceLock<HighlightedLinesCache> = OnceLock::new();

pub(crate) fn highlighted_lines_cache() -> &'static HighlightedLinesCache {
    HIGHLIGHTED_LINES_CACHE.get_or_init(|| {
        let cache_capacity = NonZeroUsize::new(200).unwrap();
        Mutex::new(LruCache::new(cache_capacity))
    })
}

fn spawn_highlight_full_file(path: PathBuf, theme: Theme, event_sender: UnboundedSender<Event>) {
    // TODO: cancel thread if app closes
    tokio::spawn(async move {
        match fs::metadata(&path) {
            Ok(metadata) => {
                const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB limit
                if metadata.len() > MAX_FILE_SIZE {
                    log::info!(
                        "File {path:?} too large for caching ({} bytes)",
                        metadata.len()
                    );
                    return;
                }
            }
            Err(e) => {
                log::error!("Error reading file metadata for {path:?}: {e}");
            }
        }

        let syntax_set = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_nonewlines);
        let full = match read_lines_range_highlighted(&path, None, None, &theme, syntax_set, true) {
            Ok(full) => full.collect(),
            Err(e) => {
                log::error!("Error highlighting file {path:?}: {e}");
                return;
            }
        };

        let cache = highlighted_lines_cache();
        let mut cache_guard = cache.lock().unwrap();
        cache_guard.put(path, full);

        // Ignore error - likely app has closed
        let _ = event_sender.send(Event::App(AppEvent::Rerender));
    });
}

fn read_lines_range_highlighted_with_cache(
    path: &Path,
    start: usize,
    end: usize,
    theme: &Theme,
    event_sender: UnboundedSender<Event>,
) -> anyhow::Result<Vec<(usize, HighlightedLine)>> {
    let cache = highlighted_lines_cache();
    let mut cache_guard = cache.lock().unwrap();

    if let Some(cached_lines) = cache_guard.get(path) {
        let lines = cached_lines
            .iter()
            .skip(start)
            .take(end - start + 1)
            .cloned()
            .collect::<Vec<_>>();
        Ok(lines)
    } else {
        drop(cache_guard);

        let syntax_set = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_nonewlines);
        let lines =
            read_lines_range_highlighted(path, Some(start), Some(end), theme, syntax_set, false)?
                .collect();

        spawn_highlight_full_file(path.to_path_buf(), theme.clone(), event_sender);

        Ok(lines)
    }
}

fn build_preview_list<'a>(
    num_lines_to_show: usize,
    selected: &SearchResultLines<'_>,
    syntax_highlighting_theme: Option<&Theme>, // None means no syntax higlighting
    true_colour: bool,
    event_sender: UnboundedSender<Event>,
) -> anyhow::Result<List<'a>> {
    let line_idx = selected.search_result.line_number - 1;
    let start = line_idx.saturating_sub(num_lines_to_show);
    let end = line_idx + num_lines_to_show;

    if let Some(theme) = syntax_highlighting_theme {
        let lines = read_lines_range_highlighted_with_cache(
            &selected.search_result.path,
            start,
            end,
            theme,
            event_sender,
        )?;
        // `num_lines_to_show - 1` because diff takes up 2 lines
        let Ok((before, cur, after)) = split_indexed_lines(lines, line_idx, num_lines_to_show - 1)
        else {
            bail!("File has changed since search (lines have changed)");
        };
        if *cur.1.iter().map(|(_, s)| s).join("") != selected.search_result.line {
            bail!("File has changed since search (lines don't match)");
        }

        let list = List::new(
            before
                .iter()
                .map(|(_, l)| regions_to_line(l, true_colour))
                .chain([
                    ListItem::new(selected.old_line_diff.clone()),
                    ListItem::new(selected.new_line_diff.clone()),
                ])
                .chain(after.iter().map(|(_, l)| regions_to_line(l, true_colour))),
        );
        if let Some(bg) = theme
            .settings
            .background
            .map(|c| to_ratatui_colour(c, true_colour))
        {
            Ok(list.bg(bg))
        } else {
            Ok(list)
        }
    } else {
        let lines = read_lines_range(&selected.search_result.path, start, end)?;
        let (before, cur, after) =
            split_indexed_lines(lines.collect::<Vec<_>>(), line_idx, num_lines_to_show - 1)?; // -1 because diff takes up 2 lines
        assert_eq!(*cur.1, selected.search_result.line);

        Ok(List::new(
            before
                .iter()
                .map(|(_, l)| to_line_plain(l))
                .chain([
                    ListItem::new(selected.old_line_diff.clone()),
                    ListItem::new(selected.new_line_diff.clone()),
                ])
                .chain(after.iter().map(|(_, l)| to_line_plain(l))),
        ))
    }
}

#[allow(clippy::type_complexity)]
fn split_indexed_lines<T>(
    indexed_lines: Vec<(usize, T)>,
    line_idx: usize,
    num_lines_to_show: usize,
) -> anyhow::Result<(Vec<(usize, T)>, (usize, T), Vec<(usize, T)>)> {
    let file_start = indexed_lines.first().context("No lines found")?.0;
    let file_end = indexed_lines.last().context("No lines found")?.0;
    let (new_start, new_end) = largest_range_centered_on(
        line_idx,
        file_start,
        file_end,
        NonZeroUsize::new(num_lines_to_show).context("preview will have height 0")?,
    )?;

    let mut filtered_lines = indexed_lines
        .into_iter()
        .skip_while(|(idx, _)| *idx < new_start)
        .take_while(|(idx, _)| *idx <= new_end)
        .collect::<Vec<_>>();

    let position = filtered_lines
        .iter()
        .position(|(idx, _)| *idx == line_idx)
        .context("Couldn't find line in file")?;
    let after = filtered_lines.split_off(position + 1);
    let current = filtered_lines.pop().unwrap();

    Ok((filtered_lines, current, after))
}

struct SearchResultLines<'a> {
    file_path: Line<'a>,
    old_line_diff: Line<'static>,
    new_line_diff: Line<'static>,
    is_primary_selected: bool,
    search_result: &'a SearchResult,
}

fn search_result<'a>(
    idx: usize,
    is_selected: bool,
    is_primary_selected: bool,
    result: &'a SearchResult,
    base_path: &Path,
    list_area_width: u16,
) -> SearchResultLines<'a> {
    let (old_line, new_line) = line_diff(&result.line, &result.replacement);
    let old_line = old_line
        .iter()
        .take(list_area_width as usize)
        .collect::<Vec<_>>();
    let new_line = new_line
        .iter()
        .take(list_area_width as usize)
        .collect::<Vec<_>>();

    SearchResultLines {
        file_path: file_path_line(
            idx,
            result,
            base_path,
            is_selected,
            is_primary_selected,
            list_area_width,
        ),
        old_line_diff: diff_to_line(old_line),
        new_line_diff: diff_to_line(new_line),
        is_primary_selected,
        search_result: result,
    }
}

static TRUNCATION_PREFIX: &str = "…";

fn file_path_line<'a>(
    idx: usize,
    result: &SearchResult,
    base_path: &Path,
    is_selected: bool,
    is_primary_selected: bool,
    list_area_width: u16,
) -> Line<'a> {
    let mut file_path_style = Style::new();
    if is_selected {
        file_path_style = file_path_style
            .bg(match (result.included, is_primary_selected) {
                (true, true) => Color::Blue,
                (true, false) => Color::Indexed(26),
                (false, true) => Color::Red,
                (false, false) => Color::Indexed(167),
            })
            .fg(Color::Indexed(255));
    }

    let right_content = format!(" ({})", idx + 1);
    let right_content_len = right_content.len();
    let left_content = format!("[{}] ", if result.included { 'x' } else { ' ' },);
    let left_content_len = left_content.chars().count();
    let centre_content = format!(
        "{}:{}",
        relative_path_from(base_path, &result.path),
        result.line_number,
    );
    let centre_content_space =
        (list_area_width as usize).saturating_sub(left_content_len + right_content_len);
    let centre_content = if centre_content.len() > centre_content_space {
        let truncated = last_n_chars(
            &centre_content,
            centre_content_space - TRUNCATION_PREFIX.chars().count(),
        );
        format!("{TRUNCATION_PREFIX}{truncated}").to_string()
    } else {
        centre_content
    };
    let spacers = " ".repeat(
        (list_area_width as usize)
            .saturating_sub(left_content_len + centre_content.len() + right_content_len),
    );

    let accessory_colour = if is_selected {
        Color::Indexed(255)
    } else {
        Color::Blue
    };
    Line::from(vec![
        Span::raw(left_content).style(accessory_colour),
        Span::raw(centre_content),
        Span::raw(spacers),
        Span::raw(right_content).style(accessory_colour),
    ])
    .style(file_path_style)
}

fn render_results_view(frame: &mut Frame<'_>, replace_state: &ReplaceState, area: Rect) {
    let area = default_width(area);
    if replace_state.errors.is_empty() {
        render_results_success(area, replace_state, frame);
    } else {
        render_results_errors(area, replace_state, frame);
    }
}

const ERROR_ITEM_HEIGHT: u16 = 3;
const NUM_TALLIES: u16 = 3;

fn render_results_success(area: Rect, replace_state: &ReplaceState, frame: &mut Frame<'_>) {
    let [_, success_title_area, results_area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(ERROR_ITEM_HEIGHT * NUM_TALLIES), // TODO: find a better way of doing this
        Constraint::Fill(1),
    ])
    .flex(Flex::Start)
    .areas(area);

    render_results_tallies(results_area, frame, replace_state);

    let text = "Success!";
    let area = center(
        success_title_area,
        Constraint::Length(u16::try_from(text.len()).unwrap_or(u16::MAX)), // TODO: find a better way of doing this
        Constraint::Length(1),
    );
    frame.render_widget(Text::styled(text, Color::Green), area);
}

fn render_results_errors(area: Rect, replace_state: &ReplaceState, frame: &mut Frame<'_>) {
    let [results_area, list_title_area, list_area] = Layout::vertical([
        Constraint::Length(ERROR_ITEM_HEIGHT * NUM_TALLIES), // TODO: find a better way of doing this
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
                        panic!("Found successful result in errors: {res:?}")
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
    let widgets: [_; NUM_TALLIES as usize] = [
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
        let area = default_width(area);
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
        .split(frame.area());
    let [header_area, content_area, footer_area] = chunks[..] else {
        panic!("Unexpected chunks length {}", chunks.len())
    };

    let title_block = Block::default().style(Style::default());
    let title = Paragraph::new(Text::styled("Scooter", Style::new().fg(Color::Blue)))
        .block(title_block)
        .alignment(Alignment::Center);
    frame.render_widget(title, header_area);

    render_key_hints(app, frame, footer_area);

    let base_path = &app.directory;
    match &mut app.current_screen {
        Screen::SearchFields => render_search_view(frame, app, content_area),
        Screen::SearchProgressing(ref mut s) => {
            render_confirmation_view(
                frame,
                false,
                &mut s.search_state,
                s.search_started.elapsed(),
                base_path,
                content_area,
                app.config.get_theme(),
                app.config.style.true_color,
                app.event_sender.clone(),
            );
        }
        Screen::SearchComplete(ref mut state) => {
            render_confirmation_view(
                frame,
                true,
                &mut state.search_state,
                state.search_time_taken,
                base_path,
                content_area,
                app.config.get_theme(),
                app.config.style.true_color,
                app.event_sender.clone(),
            );
        }
        Screen::PerformingReplacement(_) => {
            render_loading_view("Performing replacement...".to_owned())(frame, app, content_area);
        }
        Screen::Results(ref replace_state) => {
            render_results_view(frame, replace_state, content_area);
        }
    }

    match app.popup() {
        Some(Popup::Error) => render_error_popup(&app.errors(), frame, content_area),
        Some(Popup::Help) => render_help_popup(app.keymaps_all(), frame, content_area),
        None => {}
    }
}

fn render_key_hints(app: &App, frame: &mut Frame<'_>, chunk: Rect) {
    let keys_hint = Span::styled(
        app.keymaps_compact()
            .iter()
            .map(|(from, to)| format!("{from} {to}"))
            .join(" / "),
        Color::Blue,
    );

    let footer = Paragraph::new(Line::from(keys_hint))
        .block(Block::default())
        .alignment(Alignment::Center);
    frame.render_widget(footer, chunk);
}

fn render_error_popup(errors: &[AppError], frame: &mut Frame<'_>, area: Rect) {
    let error_lines: Vec<Line<'_>> = errors
        .iter()
        .enumerate()
        .flat_map(|(idx, AppError { name, long, .. })| {
            let name_line = Line::from(vec![Span::styled(name, Style::default().bold())]);

            let error_lines = long
                .lines()
                .map(|line| Line::from(vec![Span::styled(line, Style::default().fg(Color::Red))]));

            std::iter::once(name_line)
                .chain(error_lines)
                .chain(if idx < errors.len() - 1 {
                    Some(Line::from(""))
                } else {
                    None
                })
        })
        .collect();

    render_paragraph_popup("Errors", error_lines, frame, area);
}

fn render_help_popup(keymaps: Vec<(&str, String)>, frame: &mut Frame<'_>, area: Rect) {
    let max_from_width = keymaps
        .iter()
        .map(|(from, _)| from.len())
        .max()
        .unwrap_or(0);

    let from_column_width = u16::try_from(max_from_width + 2).unwrap_or(u16::MAX);

    let rows: Vec<Row<'_>> = keymaps
        .into_iter()
        .map(|(from, to)| {
            let padded_from = format!("  {from:>max_from_width$} ");

            Row::new(vec![
                Cell::from(Span::styled(padded_from, Style::default().fg(Color::Blue))),
                Cell::from(Span::raw(to)),
            ])
        })
        .collect();

    let widths = [Constraint::Length(from_column_width), Constraint::Fill(1)];
    let rows_len = rows.len();
    let table = Table::new(rows, widths).column_spacing(1);

    render_table_popup("Help", table, rows_len, frame, area);
}

fn render_paragraph_popup(title: &str, content: Vec<Line<'_>>, frame: &mut Frame<'_>, area: Rect) {
    let content_height = u16::try_from(content.len()).unwrap() + 2;
    let popup_area = get_popup_area(area, content_height);

    let popup = Paragraph::new(content).block(create_popup_block(title));
    frame.render_widget(Clear, popup_area);
    frame.render_widget(popup, popup_area);
}

fn render_table_popup(
    title: &str,
    table: Table<'_>,
    row_count: usize,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let content_height = u16::try_from(row_count + 2).unwrap_or(u16::MAX);
    let popup_area = get_popup_area(area, content_height);

    let table = table.block(create_popup_block(title));
    frame.render_widget(Clear, popup_area);
    frame.render_widget(table, popup_area);
}

fn get_popup_area(area: Rect, content_height: u16) -> Rect {
    center(
        area,
        Constraint::Percentage(75),
        Constraint::Length(content_height),
    )
}

fn create_popup_block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_style(Color::Green)
        .title(title)
        .title_alignment(Alignment::Center)
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
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Green,
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
                fg_colour: Color::Red,
                bg_colour: None,
            },
            Diff {
                text: "h".to_owned(),
                fg_colour: Color::Red,
                bg_colour: None,
            },
            Diff {
                text: "e".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Some(Color::Red),
            },
            Diff {
                text: "llo".to_owned(),
                fg_colour: Color::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "h".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "a".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Some(Color::Green),
            },
            Diff {
                text: "llo".to_owned(),
                fg_colour: Color::Green,
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
                fg_colour: Color::Red,
                bg_colour: None,
            },
            Diff {
                text: "foo".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Some(Color::Red),
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "bar".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Some(Color::Green),
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
            bg_colour: None,
        }];

        let new_expected = vec![Diff {
            text: "+ ".to_owned(),
            fg_colour: Color::Green,
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
                fg_colour: Color::Red,
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "!".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Some(Color::Green),
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
                bg_colour: None,
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Red,
                bg_colour: None,
            },
        ];

        let new_expected = vec![
            Diff {
                text: "+ ".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
            Diff {
                text: "!".to_owned(),
                fg_colour: Color::Black,
                bg_colour: Some(Color::Green),
            },
            Diff {
                text: "hello".to_owned(),
                fg_colour: Color::Green,
                bg_colour: None,
            },
        ];

        assert_eq!(old_expected, old_actual);
        assert_eq!(new_expected, new_actual);
    }

    #[test]
    fn test_split_lines_centered() {
        let lines: Vec<(usize, String)> =
            (0..=10).map(|idx| (idx, format!("Line {idx}"))).collect();

        let (before, cur, after) = split_indexed_lines(lines, 5, 5).unwrap();

        assert_eq!(cur, (5, "Line 5".to_string()));
        assert_eq!(
            before,
            vec![(3, "Line 3".to_string()), (4, "Line 4".to_string()),]
        );
        assert_eq!(
            after,
            vec![(6, "Line 6".to_string()), (7, "Line 7".to_string()),]
        );
    }

    #[test]
    fn test_split_lines_at_start() {
        let lines: Vec<(usize, String)> =
            (0..=10).map(|idx| (idx, format!("Line {idx}"))).collect();

        let (before, cur, after) = split_indexed_lines(lines, 0, 3).unwrap();

        assert_eq!(cur, (0, "Line 0".to_string()));
        assert_eq!(before, vec![]); // No lines before 0
        assert_eq!(
            after,
            vec![(1, "Line 1".to_string()), (2, "Line 2".to_string()),]
        );
    }

    #[test]
    fn test_split_lines_at_end() {
        let lines: Vec<(usize, String)> =
            (0..=10).map(|idx| (idx, format!("Line {idx}"))).collect();

        let (before, cur, after) = split_indexed_lines(lines, 10, 3).unwrap();

        assert_eq!(cur, (10, "Line 10".to_string()));
        assert_eq!(
            before,
            vec![(8, "Line 8".to_string()), (9, "Line 9".to_string()),]
        );
        assert_eq!(after, vec![]);
    }

    #[test]
    fn test_split_lines_with_small_window() {
        let lines: Vec<(usize, String)> =
            (0..=10).map(|idx| (idx, format!("Line {idx}"))).collect();

        let (before, cur, after) = split_indexed_lines(lines, 5, 1).unwrap();

        assert_eq!(cur, (5, "Line 5".to_string()));
        assert_eq!(before, vec![]);
        assert_eq!(after, vec![]);
    }

    #[test]
    fn test_split_lines_with_custom_data_type() {
        let lines = (0..=5)
            .map(|idx| (idx, vec![idx, (idx * 2)]))
            .collect::<Vec<_>>();

        let (before, cur, after) = split_indexed_lines(lines, 3, 2).unwrap();

        assert_eq!(cur, (3, vec![3, 6]));
        assert_eq!(before, vec![]);
        assert_eq!(after, vec![(4, vec![4, 8])]);
    }

    #[test]
    fn test_split_lines_non_sequential_indices() {
        let lines: Vec<(usize, &str)> = vec![
            (10, "Line 10"),
            (20, "Line 20"),
            (30, "Line 30"),
            (40, "Line 40"),
            (50, "Line 50"),
        ];

        let (before, cur, after) = split_indexed_lines(lines, 30, 3).unwrap();

        assert_eq!(cur, (30, "Line 30"));
        // Both before and after should be empty - `num_lines_to_show` should be just sequential lines
        assert_eq!(before, vec![]);
        assert_eq!(after, vec![]);
    }

    #[test]
    fn test_split_lines_non_sequential_indices_large_window() {
        let lines: Vec<(usize, &str)> = vec![
            (10, "Line 10"),
            (20, "Line 20"),
            (30, "Line 30"),
            (40, "Line 40"),
            (50, "Line 50"),
        ];

        let (before, cur, after) = split_indexed_lines(lines, 30, 30).unwrap();

        assert_eq!(cur, (30, "Line 30"));
        assert_eq!(before, vec![(20, "Line 20")]);
        assert_eq!(after, vec![(40, "Line 40")]);
    }

    #[test]
    #[should_panic(expected = "Expected start<=pos<=end, found start=0, pos=10, end=5")]
    fn test_split_lines_line_idx_not_found() {
        let lines: Vec<(usize, String)> = (0..=5).map(|idx| (idx, format!("Line {idx}"))).collect();

        let _ = split_indexed_lines(lines, 10, 3).unwrap();
        // Should panic because line 10 is not in the data
    }
}
