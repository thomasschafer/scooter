use anyhow::{anyhow, bail};
use itertools::Itertools;
use lru::LruCache;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Flex, Layout, Position, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Cell, Clear, List, ListItem, Padding, Paragraph, Row, Table},
};
use scooter_core::{
    app::{App, AppEvent, Event, FocussedSection, InputSource, Popup, Screen, SearchState},
    diff::{Diff, DiffColour, line_diff},
    errors::AppError,
    fields::{Field, NUM_SEARCH_FIELDS, SearchField, SearchFields},
    replace::{PerformingReplacementState, ReplaceState},
    utils::{
        self, HighlightedLine, last_n_chars, read_lines_range_highlighted, relative_path,
        strip_control_chars,
    },
};
use std::{
    borrow::Cow,
    cmp::min,
    fs,
    io::Cursor,
    iter,
    num::NonZeroUsize,
    ops::Div,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, atomic::Ordering},
    time::Duration,
};
use syntect::{
    highlighting::{FontStyle, Style as SyntectStyle, Theme},
    parsing::SyntaxSet,
};
use tokio::sync::mpsc::UnboundedSender;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use frep_core::search::SearchResultWithReplacement;
use scooter_core::{config::Config, utils::read_lines_range};

use super::colour::to_ratatui_colour;

fn create_title_spans<'a>(
    field: &SearchField,
    title: &'a str,
    highlighted: bool,
    set_by_cli: bool,
    disable_prepopulated_fields: bool,
) -> Vec<Span<'a>> {
    let mut fg_color = Color::Reset;
    if set_by_cli && disable_prepopulated_fields {
        fg_color = Color::Blue;
    } else if highlighted {
        fg_color = Color::Green;
    }
    let title_style = Style::new().fg(fg_color);

    let mut spans = vec![Span::styled(title, title_style)];
    if let Some(error) = field.error() {
        spans.push(Span::styled(
            format!(" (Error: {})", error.short),
            Style::new().fg(Color::Red),
        ));
    }
    spans
}

pub fn render_search_field(
    field: &SearchField,
    frame: &mut Frame<'_>,
    area: Rect,
    highlighted: bool,
    disable_prepopulated_fields: bool,
) {
    let mut block = Block::bordered();
    if field.set_by_cli && disable_prepopulated_fields {
        block = block.border_style(Style::new().blue());
    } else if highlighted {
        block = block.border_style(Style::new().green());
    }

    let title_spans = create_title_spans(
        field,
        field.name.title(),
        highlighted,
        field.set_by_cli,
        disable_prepopulated_fields,
    );

    match &field.field {
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

static SEARCH_FIELD_HEIGHT: u16 = 3;
static NUM_SEARCH_FIELDS_TRUNCATED: u16 = 2;

fn render_search_fields(
    frame: &mut Frame<'_>,
    search_fields: &SearchFields,
    config: &Config,
    show_popup: bool,
    num_search_fields_to_render: u16,
    is_focussed: bool,
    area: Rect,
) {
    let areas = Layout::vertical(iter::repeat_n(
        Constraint::Length(SEARCH_FIELD_HEIGHT),
        num_search_fields_to_render as usize,
    ))
    .flex(Flex::Center)
    .split(area);

    search_fields
        .fields
        .iter()
        .zip(areas.iter())
        .enumerate()
        .for_each(|(idx, (search_field, &field_area))| {
            render_search_field(
                search_field,
                frame,
                field_area,
                is_focussed && idx == search_fields.highlighted,
                config.search.disable_prepopulated_fields,
            );
        });

    if is_focussed && !show_popup {
        let field = search_fields.highlighted_field();
        if !(field.set_by_cli && config.search.disable_prepopulated_fields)
            && let Some(cursor_pos) = field.cursor_pos()
        {
            let highlighted_area = areas[search_fields.highlighted];

            frame.set_cursor_position(Position {
                x: highlighted_area.x + u16::try_from(cursor_pos).unwrap_or(0) + 1,
                y: highlighted_area.y + 1,
            });
        }
    }
}

fn default_width(area: Rect) -> Rect {
    let width_percentage = if area.width >= 300 { 80 } else { 90 };
    width(area, width_percentage)
}

fn width(area: Rect, percentage: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Percentage(percentage)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn diff_col_to_ratatui(colour: &DiffColour) -> Color {
    match colour {
        DiffColour::Red => Color::Red,
        DiffColour::Green => Color::Green,
        DiffColour::Black => Color::Black,
    }
}

fn diff_to_line<'a>(diff: impl Iterator<Item = &'a Diff>) -> StyledLine {
    diff.map(|d| {
        let mut style = Style::new().fg(diff_col_to_ratatui(&d.fg_colour));
        if let Some(bg) = &d.bg_colour {
            style = style.bg(diff_col_to_ratatui(bg));
        }
        (Cow::Owned(strip_control_chars(&d.text)), Some(style))
    })
    .collect()
}

fn display_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let milliseconds = duration.subsec_millis();
    format!("{seconds}.{milliseconds:03}s")
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::fn_params_excessive_bools
)]
fn render_search_results(
    frame: &mut Frame<'_>,
    input_source: &InputSource,
    is_complete: bool,
    search_state: &mut SearchState,
    time_taken: Duration,
    area: Rect,
    theme: Option<&Theme>,
    true_colour: bool,
    event_sender: UnboundedSender<Event>,
    area_is_focussed: bool,
    preview_update_status: Option<(usize, usize)>,
    wrap: bool,
) {
    let small_screen = area.width <= 110;

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
        preview_update_status,
    );

    let num_to_render = if small_screen {
        5
    } else {
        results_area.height as usize
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

    let (list_area, preview_area) = if small_screen {
        let [list_area, _, preview_area] = Layout::vertical([
            #[allow(clippy::cast_possible_truncation)]
            Constraint::Length(num_to_render as u16),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(results_area);
        (list_area, preview_area)
    } else {
        let [list_area, _, preview_area] = Layout::horizontal([
            Constraint::Fill(2),
            Constraint::Length(1),
            Constraint::Fill(3),
        ])
        .areas(results_area);
        (list_area, preview_area)
    };

    let base_path = match &input_source {
        InputSource::Directory(dir) => dir,
        InputSource::Stdin(_) => &PathBuf::from("."),
    };
    let search_results = build_search_results(
        search_state,
        base_path,
        list_area.width,
        num_to_render,
        area_is_focussed,
    );
    let search_results_list = search_results
        .iter()
        .map(|SearchResultLines { file_path, .. }| ListItem::new(file_path.clone()));
    frame.render_widget(List::new(search_results_list), list_area);

    if !search_results.is_empty() {
        let selected = search_results
            .iter()
            .find(|s| s.is_primary_selected)
            .expect("Selected item should be in view");
        let lines_to_show = preview_area.height;

        match build_preview_list(
            input_source,
            lines_to_show,
            selected,
            theme,
            true_colour,
            event_sender,
            if wrap {
                WrapText::Width {
                    width: preview_area.width,
                    num_lines: lines_to_show,
                }
            } else {
                WrapText::None
            },
        ) {
            Ok(preview) => {
                frame.render_widget(preview, preview_area);
            }
            Err(e) => {
                frame.render_widget(
                    Paragraph::new(format!("Error generating preview: {e}")).fg(Color::Red),
                    preview_area,
                );
            }
        }
    }
}

fn render_num_results(
    frame: &mut Frame<'_>,
    area: Rect,
    num_results: usize,
    is_complete: bool,
    time_taken: Duration,
    num_replacements_updates_in_progress: Option<(usize, usize)>,
) {
    let left_content_1 = format!("Results: {num_results}");
    let left_content_2 = if is_complete {
        " [Search complete]"
    } else {
        " [Still searching...]"
    };
    let mid_content = preview_update_status(num_replacements_updates_in_progress);
    let right_content = format!(" [Time taken: {}]", display_duration(time_taken));
    let num_total_spacers = (area.width as usize).saturating_sub(
        left_content_1.len() + left_content_2.len() + mid_content.len() + right_content.len(),
    );
    let spacers_each_side = " ".repeat(num_total_spacers / 2);

    let accessory_colour = if is_complete {
        Color::Green
    } else {
        Color::Blue
    };

    frame.render_widget(
        Line::from(vec![
            Span::raw(left_content_1),
            Span::raw(left_content_2).fg(accessory_colour),
            Span::raw(spacers_each_side.clone()),
            Span::raw(mid_content).fg(Color::Blue),
            Span::raw(spacers_each_side),
            Span::raw(right_content).fg(accessory_colour),
        ]),
        area,
    );
}

fn preview_update_status(num_replacements_updates_in_progress: Option<(usize, usize)>) -> String {
    if let Some((complete, total)) = num_replacements_updates_in_progress {
        // Avoid flickering - only show if it will take some time
        if total >= 10_000 {
            #[allow(clippy::cast_precision_loss)]
            return format!(
                "[Updating preview: {complete}/{total} ({perc:.2}%)]",
                perc = ((complete as f64) / (total as f64)) * 100.0
            );
        }
    }

    String::new()
}

fn build_search_results<'a>(
    search_state: &'a mut SearchState,
    base_path: &Path,
    width: u16,
    num_to_render: usize,
    area_is_focussed: bool,
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
                area_is_focussed,
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

type StyledLine = Vec<(Cow<'static, str>, Option<Style>)>;

static PREVIEW_LINE_PREFIX: &str = "  ";
static WRAPPED_LINE_PREFIX: &str = "  ↪ ";

fn regions_to_line(line: &[(Option<SyntectStyle>, String)], true_colour: bool) -> StyledLine {
    iter::once((Cow::Borrowed(PREVIEW_LINE_PREFIX), None))
        .chain(line.iter().map(|(style, s)| {
            (
                Cow::Owned(strip_control_chars(s)),
                style
                    .as_ref()
                    .map(|style| convert_syntect_to_ratatui_style(style, true_colour)),
            )
        }))
        .collect()
}

fn to_line_plain(line: &str) -> StyledLine {
    vec![(
        Cow::Owned(format!(
            "{PREVIEW_LINE_PREFIX}{}",
            strip_control_chars(line)
        )),
        None,
    )]
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
    tokio::spawn(async move {
        match fs::metadata(&path) {
            Ok(metadata) => {
                const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB limit
                if metadata.len() > MAX_FILE_SIZE {
                    log::info!(
                        "File {} too large for caching ({} bytes)",
                        path.display(),
                        metadata.len()
                    );
                    return;
                }
            }
            Err(e) => {
                log::error!("Error reading file metadata for {}: {e}", path.display());
            }
        }

        let syntax_set = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_nonewlines);
        let full = match read_lines_range_highlighted(&path, None, None, &theme, syntax_set, true) {
            Ok(full) => full.collect(),
            Err(e) => {
                log::error!("Error highlighting file {}: {e}", path.display());
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum WrapText {
    None,
    Width { width: u16, num_lines: u16 },
}

fn build_preview_list<'a>(
    input_source: &InputSource,
    num_lines_to_show: u16,
    selected: &SearchResultLines<'_>,
    syntax_highlighting_theme: Option<&Theme>, // None means no syntax higlighting
    true_colour: bool,
    event_sender: UnboundedSender<Event>,
    wrap: WrapText,
) -> anyhow::Result<List<'a>> {
    match input_source {
        InputSource::Directory(_) => build_preview_from_file(
            num_lines_to_show,
            selected,
            syntax_highlighting_theme,
            true_colour,
            event_sender,
            wrap,
        ),
        InputSource::Stdin(stdin) => {
            build_preview_from_str(stdin, num_lines_to_show, selected, wrap)
        }
    }
}

fn build_preview_from_str<'a>(
    stdin: &Arc<String>,
    num_lines_to_show: u16,
    selected: &SearchResultLines<'_>,
    wrap: WrapText,
) -> anyhow::Result<List<'a>> {
    // Line numbers are 1-indexed
    let line_idx = selected.result.search_result.line_number - 1;
    let start = line_idx.saturating_sub(num_lines_to_show as usize);
    let end = line_idx + num_lines_to_show as usize;

    let cursor = Cursor::new(stdin.as_bytes());
    let lines = utils::surrounding_line_window(cursor, start, end).collect();

    // `num_lines_to_show - 1` because diff takes up 2 lines
    let (before, cur, after) =
        utils::split_indexed_lines(lines, line_idx, num_lines_to_show.saturating_sub(1))?;
    assert!(
        *cur.1 == selected.result.search_result.line,
        "Expected line didn't match actual",
    );

    let before = before.iter().map(|(_, l)| to_line_plain(l));
    let diff = [
        selected.old_line_diff.clone(),
        selected.new_line_diff.clone(),
    ];
    let after = after.iter().map(|(_, l)| to_line_plain(l));
    line_list(before, diff, after, num_lines_to_show, wrap)
        .map_err(|e| anyhow!("failed to combine lines: {e}"))
}

fn styled_line_to_ratatui_line(line: StyledLine) -> ListItem<'static> {
    let spans: Vec<Span<'_>> = line
        .into_iter()
        .map(|(text, style)| {
            let span = Span::raw(text);
            match style {
                Some(s) => span.style(s),
                None => span,
            }
        })
        .collect();
    ListItem::new(Line::from(spans))
}

fn wrap_lines(
    line: impl IntoIterator<Item = StyledLine>,
    width: u16,
    num_lines: Option<u16>,
) -> Vec<StyledLine> {
    let wrapped_line_prefix_len = UnicodeWidthStr::width(WRAPPED_LINE_PREFIX);
    if width as usize <= wrapped_line_prefix_len || num_lines.is_some_and(|n| n == 0) {
        return vec![];
    }
    let mut line = line.into_iter();

    let mut wrapped_line: Vec<StyledLine> = vec![];
    // Reversed full line, so that we can pop elements
    let mut next_line_stack: StyledLine = vec![];

    loop {
        if num_lines.is_some_and(|n| wrapped_line.len() >= n as usize) {
            break;
        }
        let include_prefix = if next_line_stack.is_empty() {
            match line.next() {
                Some(mut n) => {
                    n.reverse();
                    next_line_stack = n;
                    false
                }
                None => break,
            }
        } else {
            true
        };

        let mut cur_line_wrapped: StyledLine = vec![];
        let mut cur_line_wrapped_len: usize = 0;
        if include_prefix {
            cur_line_wrapped.push((WRAPPED_LINE_PREFIX.into(), Some(Style::default().dim())));
            cur_line_wrapped_len += UnicodeWidthStr::width(WRAPPED_LINE_PREFIX);
        }

        #[allow(clippy::needless_continue)]
        while cur_line_wrapped_len < width as usize {
            let Some((next_seg_chars, next_seg_style)) = next_line_stack.pop() else {
                break;
            };
            if next_seg_chars.is_empty() {
                continue;
            }

            // Grab chunk from next_seg, add rest back for processing later
            let (next_seg_chars, rest) = split_first_chunk(&next_seg_chars);
            if !rest.is_empty() {
                next_line_stack.push((Cow::Owned(rest.to_string()), next_seg_style));
            }
            assert!(!next_seg_chars.is_empty());

            let next_seg_len = UnicodeWidthStr::width(next_seg_chars);
            if cur_line_wrapped_len + next_seg_len <= width as usize {
                // Fits on the current line
                cur_line_wrapped_len += next_seg_len;
                cur_line_wrapped.push((Cow::Owned(next_seg_chars.to_string()), next_seg_style));
            } else if next_seg_len + wrapped_line_prefix_len <= width as usize {
                // Fits on next line
                next_line_stack.push((Cow::Owned(next_seg_chars.to_string()), next_seg_style));
                break;
            } else {
                // Wider than an entire line, so break it up over this line and the next
                let (first_part, rest) = extract_first_n_width(
                    next_seg_chars,
                    (width as usize).saturating_sub(cur_line_wrapped_len),
                );
                // If we wouldn't make progress, grab out the first grapheme (even if it won't fit fully)
                // to ensure we don't get stuck in a loop
                let (first_part, rest) = if first_part.is_empty() {
                    let mut graphemes = next_seg_chars.graphemes(true);
                    let Some(first_part) = graphemes.next() else {
                        continue;
                    };
                    (first_part.to_string(), graphemes.collect())
                } else {
                    (first_part.to_string(), rest.to_string())
                };
                for part in [rest, first_part] {
                    next_line_stack.push((Cow::Owned(part), next_seg_style));
                }
                continue;
            }
        }

        wrapped_line.push(cur_line_wrapped);
    }
    wrapped_line
}

/// Split a string at the boundary between alphabetic and non-alphabetic grapheme clusters.
/// This respects grapheme cluster boundaries to avoid splitting e.g. emojis with modifiers.
fn split_first_chunk(s: &str) -> (&str, &str) {
    let mut graphemes = s.graphemes(true);
    let Some(first_grapheme) = graphemes.next() else {
        return ("", "");
    };

    // Check if the first character of the grapheme is alphabetic
    let first_is_alpha = first_grapheme
        .chars()
        .next()
        .is_some_and(char::is_alphabetic);

    let mut byte_pos = first_grapheme.len();

    for grapheme in graphemes {
        let grapheme_is_alpha = grapheme.chars().next().is_some_and(char::is_alphabetic);

        if grapheme_is_alpha != first_is_alpha {
            return (&s[..byte_pos], &s[byte_pos..]);
        }
        byte_pos += grapheme.len();
    }

    (s, "")
}

/// Extract the first `max_width` display-width worth of grapheme clusters from a string.
/// This won't break up multi-codepoint graphemes e.g. emojis with skin tones.
///
/// Returns a tuple of (`first_part`, `remainder`).
fn extract_first_n_width(text: &str, max_width: usize) -> (&str, &str) {
    let mut cur_sum: usize = 0;
    let mut last_valid_byte_idx = 0;

    for grapheme in text.graphemes(true) {
        let width = UnicodeWidthStr::width(grapheme);
        if cur_sum + width > max_width {
            return (&text[..last_valid_byte_idx], &text[last_valid_byte_idx..]);
        }
        cur_sum += width;
        last_valid_byte_idx += grapheme.len();
    }
    (text, "")
}

#[allow(clippy::needless_pass_by_value)]
fn line_list(
    before: impl IntoIterator<Item = StyledLine>,
    diff: impl IntoIterator<Item = StyledLine>,
    after: impl IntoIterator<Item = StyledLine>,
    num_lines_to_show: u16,
    wrap: WrapText,
) -> anyhow::Result<List<'static>> {
    let lines: Box<dyn Iterator<Item = StyledLine>> = match wrap {
        WrapText::Width { width, num_lines } => {
            let wrapped_diff = wrap_lines(diff, width, Some(num_lines));

            let remaining_lines = num_lines_to_show
                .saturating_sub(u16::try_from(wrapped_diff.len()).unwrap_or(u16::MAX));

            // TODO: ideally we'd process from the back to avoid the need for the `last_n` call, but this
            // adds a lot of complexity. Can revisit if needed
            let wrapped_before =
                utils::last_n(&wrap_lines(before, width, None), remaining_lines as usize).to_vec();

            let wrapped_after = wrap_lines(after, width, Some(remaining_lines));

            // Get a window centered around the diff
            let line_idx = wrapped_before.len() + wrapped_diff.len().div(2).saturating_sub(1);
            let (before, cur, after) = utils::split_indexed_lines(
                wrapped_before
                    .into_iter()
                    .chain(wrapped_diff)
                    .chain(wrapped_after)
                    .enumerate()
                    .collect(),
                line_idx,
                num_lines_to_show,
            )?;

            Box::new(
                before
                    .into_iter()
                    .chain(vec![cur])
                    .chain(after)
                    .map(|(_, x)| x),
            )
        }
        WrapText::None => Box::new(before.into_iter().chain(diff).chain(after)),
    };
    Ok(List::new(lines.map(styled_line_to_ratatui_line)))
}

fn build_preview_from_file<'a>(
    num_lines_to_show: u16,
    selected: &SearchResultLines<'_>,
    syntax_highlighting_theme: Option<&Theme>,
    true_colour: bool,
    event_sender: UnboundedSender<Event>,
    wrap: WrapText,
) -> anyhow::Result<List<'a>> {
    let path = selected
        .result
        .search_result
        .path
        .as_ref()
        .expect("attempted to build preview list from file with no path");

    // Line numbers are 1-indexed
    let line_idx = selected.result.search_result.line_number - 1;
    let start = line_idx.saturating_sub(num_lines_to_show as usize);
    let end = line_idx + num_lines_to_show as usize;

    if let Some(theme) = syntax_highlighting_theme {
        let lines = read_lines_range_highlighted_with_cache(path, start, end, theme, event_sender)?;
        // `num_lines_to_show - 1` because diff takes up 2 lines
        let Ok((before, cur, after)) =
            utils::split_indexed_lines(lines, line_idx, num_lines_to_show.saturating_sub(1))
        else {
            bail!("File has changed since search (lines have changed)");
        };
        if *cur.1.iter().map(|(_, s)| s).join("") != selected.result.search_result.line {
            bail!("File has changed since search (lines don't match)");
        }

        let before = before.iter().map(|(_, l)| regions_to_line(l, true_colour));
        let diff = [
            selected.old_line_diff.clone(),
            selected.new_line_diff.clone(),
        ];
        let after = after.iter().map(|(_, l)| regions_to_line(l, true_colour));
        let list = line_list(before, diff, after, num_lines_to_show, wrap)
            .map_err(|e| anyhow!("failed to combine lines: {e}"))?;
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
        let lines = read_lines_range(path, start, end)?.collect();
        // `num_lines_to_show - 1` because diff takes up 2 lines
        let Ok((before, cur, after)) =
            utils::split_indexed_lines(lines, line_idx, num_lines_to_show.saturating_sub(1))
        else {
            bail!("File has changed since search (lines have changed)");
        };
        if *cur.1 != selected.result.search_result.line {
            bail!("File has changed since search (lines don't match)");
        }

        let before = before.iter().map(|(_, l)| to_line_plain(l));
        let diff = [
            selected.old_line_diff.clone(),
            selected.new_line_diff.clone(),
        ];
        let after = after.iter().map(|(_, l)| to_line_plain(l));
        line_list(before, diff, after, num_lines_to_show, wrap)
            .map_err(|e| anyhow!("failed to combine lines: {e}"))
    }
}

struct SearchResultLines<'a> {
    file_path: Line<'a>,
    old_line_diff: StyledLine,
    new_line_diff: StyledLine,
    is_primary_selected: bool,
    result: &'a SearchResultWithReplacement,
}

fn search_result<'a>(
    idx: usize,
    is_selected: bool,
    is_primary_selected: bool,
    result: &'a SearchResultWithReplacement,
    base_path: &Path,
    list_area_width: u16,
    area_is_focussed: bool,
) -> SearchResultLines<'a> {
    let (old_line, new_line) = line_diff(&result.search_result.line, &result.replacement);
    let old_line = old_line.iter().take(list_area_width as usize);
    let new_line = new_line.iter().take(list_area_width as usize);

    SearchResultLines {
        file_path: file_path_line(
            idx,
            result,
            base_path,
            is_selected,
            is_primary_selected,
            list_area_width,
            area_is_focussed,
        ),
        old_line_diff: diff_to_line(old_line),
        new_line_diff: diff_to_line(new_line),
        is_primary_selected,
        result,
    }
}

static TRUNCATION_PREFIX: &str = "…";

fn file_path_line<'a>(
    idx: usize,
    result: &SearchResultWithReplacement,
    base_path: &Path,
    is_selected: bool,
    is_primary_selected: bool,
    list_area_width: u16,
    area_is_focussed: bool,
) -> Line<'a> {
    let mut file_path_style = Style::new();
    if area_is_focussed && is_selected {
        file_path_style = file_path_style
            .bg(match (result.search_result.included, is_primary_selected) {
                (true, true) => Color::Blue,
                (true, false) => Color::Indexed(26),
                (false, true) => Color::Red,
                (false, false) => Color::Indexed(167),
            })
            .fg(Color::Indexed(255));
    }

    let right_content = format!(" ({})", idx + 1);
    let right_content_len = right_content.chars().count();
    let left_content = format!(
        "[{}] ",
        if result.search_result.included {
            'x'
        } else {
            ' '
        },
    );
    let left_content_len = left_content.chars().count();
    let mut path = match &result.search_result.path {
        Some(path) => relative_path(base_path, path),
        None => "stdin".to_string(),
    };
    let line_num = format!(":{}", result.search_result.line_number);
    let line_num_len = line_num.chars().count();
    let path_space = (list_area_width as usize)
        .saturating_sub(left_content_len + line_num_len + right_content_len);
    if UnicodeWidthStr::width(path.as_str()) > path_space {
        let truncated = last_n_chars(
            &path,
            path_space.saturating_sub(TRUNCATION_PREFIX.chars().count()),
        );
        path = format!("{TRUNCATION_PREFIX}{truncated}").to_string();
    }
    let path_len = UnicodeWidthStr::width(path.as_str());
    let spacers = " ".repeat(
        (list_area_width as usize)
            .saturating_sub(left_content_len + path_len + line_num_len + right_content_len),
    );

    let accessory_colour = if area_is_focussed && is_selected {
        Color::Indexed(255)
    } else {
        Color::Blue
    };
    Line::from(vec![
        Span::raw(left_content).style(accessory_colour),
        Span::raw(path),
        Span::raw(line_num).style(accessory_colour),
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
        .skip(replace_state.replacement_errors_pos)
        .take(list_area.height as usize / 3 + 1)
        .map(error_result);

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
            "Successful replacements (lines):",
            replace_state.num_successes,
            success_area,
        ),
        ("Ignored (lines):", replace_state.num_ignored, ignored_area),
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

fn render_performing_replacement_view(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &PerformingReplacementState,
) {
    let area = default_width(area);

    let [progress_area, _, stats_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .flex(Flex::Center)
    .areas(area);

    let text = Paragraph::new(Line::from(Span::raw("Performing replacement...")))
        .block(Block::default())
        .alignment(Alignment::Center);

    frame.render_widget(text, progress_area);

    let num_completed = state.num_replacements_completed.load(Ordering::Relaxed);
    let time_taken = state.replacement_started.elapsed();

    #[allow(clippy::cast_precision_loss)]
    let stats_text = format!(
        "Completed: {}/{} ({:.2}%)\nTime: {}",
        num_completed,
        state.total_replacements,
        (num_completed as f64) / (state.total_replacements.max(1) as f64) * 100.0,
        display_duration(time_taken)
    );

    frame.render_widget(
        Paragraph::new(stats_text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Blue)),
        stats_area,
    );
}

fn error_result(result: &SearchResultWithReplacement) -> [ratatui::widgets::ListItem<'static>; 3] {
    let (path_display, error) = result.display_error();

    [
        ("".to_owned(), Style::default()),
        (path_display, Style::default()),
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
    let title = Paragraph::new(Text::styled("scooter", Style::new().fg(Color::Blue)))
        .block(title_block)
        .alignment(Alignment::Center);
    frame.render_widget(title, header_area);

    render_key_hints(app, frame, footer_area);

    let show_popup = app.show_popup();
    match &mut app.current_screen {
        Screen::SearchFields(search_fields_state) => {
            let num_search_fields_to_render = match search_fields_state.focussed_section {
                FocussedSection::SearchFields => NUM_SEARCH_FIELDS,
                FocussedSection::SearchResults => NUM_SEARCH_FIELDS_TRUNCATED,
            };
            let area = default_width(content_area);
            let [fields, _, results] = Layout::vertical([
                Constraint::Length(num_search_fields_to_render * SEARCH_FIELD_HEIGHT),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .flex(Flex::Center)
            .areas(area);

            render_search_fields(
                frame,
                &app.search_fields,
                &app.config,
                show_popup,
                num_search_fields_to_render,
                search_fields_state.focussed_section == FocussedSection::SearchFields,
                fields,
            );

            let replacements_in_progress = search_fields_state.replacements_in_progress();
            if let Some(state) = &mut search_fields_state.search_state {
                let (is_complete, elapsed) = if let Some(completed) = state.search_completed {
                    (true, completed.duration_since(state.search_started))
                } else {
                    (false, state.search_started.elapsed())
                };
                render_search_results(
                    frame,
                    &app.input_source,
                    is_complete,
                    state,
                    elapsed,
                    results,
                    app.config.get_theme(),
                    app.config.style.true_color,
                    app.event_sender.clone(),
                    search_fields_state.focussed_section == FocussedSection::SearchResults,
                    replacements_in_progress,
                    app.config.preview.wrap_text,
                );
            }
        }
        Screen::PerformingReplacement(state) => {
            render_performing_replacement_view(frame, content_area, state);
        }
        Screen::Results(replace_state) => {
            render_results_view(frame, replace_state, content_area);
        }
    }

    match app.popup() {
        Some(Popup::Error) => render_error_popup(&app.errors(), frame, content_area),
        Some(Popup::Help) => render_help_popup(app.keymaps_all(), frame, content_area),
        Some(Popup::Text { title, body }) => {
            render_text_popup(title, body, frame, content_area);
        }

        None => {}
    }
}

fn render_text_popup(title: &str, body: &str, frame: &mut Frame<'_>, area: Rect) {
    let lines = body.lines().map(Line::from).collect();
    render_paragraph_popup(title, lines, frame, area);
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

fn render_help_popup(keymaps: Vec<(String, String)>, frame: &mut Frame<'_>, area: Rect) {
    let max_from_width = keymaps
        .iter()
        .map(|(from, _)| from.len())
        .max()
        .unwrap_or(0);

    let from_column_width = u16::try_from(max_from_width + 2).unwrap_or(u16::MAX);

    let rows: Vec<Row<'_>> = keymaps
        .into_iter()
        .map(|(from, to)| {
            let padded_from = format!(" {from:>max_from_width$} ");

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
        .padding(Padding::horizontal(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_lines_centered() {
        let lines: Vec<(usize, String)> =
            (0..=10).map(|idx| (idx, format!("Line {idx}"))).collect();

        let (before, cur, after) = utils::split_indexed_lines(lines, 5, 5).unwrap();

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

        let (before, cur, after) = utils::split_indexed_lines(lines, 0, 3).unwrap();

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

        let (before, cur, after) = utils::split_indexed_lines(lines, 10, 3).unwrap();

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

        let (before, cur, after) = utils::split_indexed_lines(lines, 5, 1).unwrap();

        assert_eq!(cur, (5, "Line 5".to_string()));
        assert_eq!(before, vec![]);
        assert_eq!(after, vec![]);
    }

    #[test]
    fn test_split_lines_with_custom_data_type() {
        let lines = (0..=5)
            .map(|idx| (idx, vec![idx, (idx * 2)]))
            .collect::<Vec<_>>();

        let (before, cur, after) = utils::split_indexed_lines(lines, 3, 2).unwrap();

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

        let (before, cur, after) = utils::split_indexed_lines(lines, 30, 3).unwrap();

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

        let (before, cur, after) = utils::split_indexed_lines(lines, 30, 30).unwrap();

        assert_eq!(cur, (30, "Line 30"));
        assert_eq!(before, vec![(20, "Line 20")]);
        assert_eq!(after, vec![(40, "Line 40")]);
    }

    #[test]
    #[should_panic(expected = "Expected start<=pos<=end, found start=0, pos=10, end=5")]
    fn test_split_lines_line_idx_not_found() {
        let lines: Vec<(usize, String)> = (0..=5).map(|idx| (idx, format!("Line {idx}"))).collect();

        let _ = utils::split_indexed_lines(lines, 10, 3).unwrap();
        // Should panic because line 10 is not in the data
    }

    #[test]
    fn test_split_first_chunk_words() {
        assert_eq!(split_first_chunk("foo bar"), ("foo", " bar"));
        assert_eq!(split_first_chunk(" bar"), (" ", "bar"));
        assert_eq!(split_first_chunk("bar"), ("bar", ""));
    }

    #[test]
    fn test_split_first_chunk_with_punctuation() {
        assert_eq!(
            split_first_chunk("?! some-thing..."),
            ("?! ", "some-thing...")
        );
        assert_eq!(split_first_chunk("some-thing...?"), ("some", "-thing...?"));
        assert_eq!(split_first_chunk("-thing...?"), ("-", "thing...?"));
        assert_eq!(split_first_chunk("thing...?"), ("thing", "...?"));
        assert_eq!(split_first_chunk("...?"), ("...?", ""));
    }

    #[test]
    fn test_split_first_chunk_empty() {
        assert_eq!(split_first_chunk(""), ("", ""));
    }

    #[test]
    fn test_split_first_chunk_with_emojis() {
        // Emojis and spaces are both non-alphabetic, so they group together
        assert_eq!(split_first_chunk("✅ PASSED"), ("✅ ", "PASSED"));
        assert_eq!(split_first_chunk("PASSED ✅"), ("PASSED", " ✅"));
        assert_eq!(split_first_chunk("✅✨🎉"), ("✅✨🎉", ""));
        assert_eq!(split_first_chunk("✅ test"), ("✅ ", "test"));
    }

    #[test]
    fn test_split_first_chunk_with_unicode() {
        assert_eq!(split_first_chunk("日本語 test"), ("日本語", " test"));
        assert_eq!(split_first_chunk("test 日本語"), ("test", " 日本語"));
        assert_eq!(split_first_chunk("café"), ("café", ""));
        assert_eq!(split_first_chunk("🔥code"), ("🔥", "code"));
    }

    #[test]
    fn test_split_first_chunk_mixed() {
        // Non-alphabetic characters (including emojis and spaces) group together
        assert_eq!(split_first_chunk("⚠️  warning"), ("⚠️  ", "warning"));
        assert_eq!(split_first_chunk("❌ failed test"), ("❌ ", "failed test"));
        assert_eq!(split_first_chunk("test ⚡️ fast"), ("test", " ⚡️ fast"));
    }

    #[test]
    fn test_split_first_chunk_emoji_with_skin_tone() {
        // Critical: Emoji with skin tone modifier should NOT be split
        // The old char-based implementation would have split between 👍 and 🏻
        assert_eq!(split_first_chunk("👍🏻test"), ("👍🏻", "test"));
        assert_eq!(split_first_chunk("👍🏻 code"), ("👍🏻 ", "code"));

        // Multiple emojis with modifiers
        assert_eq!(split_first_chunk("👋🏽👍🏻word"), ("👋🏽👍🏻", "word"));
    }

    #[test]
    fn test_split_first_chunk_family_emoji() {
        // Family emoji is composed of multiple codepoints with zero-width joiners
        // Must be kept together as a single unit
        assert_eq!(split_first_chunk("👨‍👩‍👧‍👦family"), ("👨‍👩‍👧‍👦", "family"));
        assert_eq!(split_first_chunk("👨‍👩‍👧‍👦 test"), ("👨‍👩‍👧‍👦 ", "test"));
    }

    #[test]
    fn test_split_first_chunk_combining_diacritics() {
        // Combining diacritics should stay with their base character
        // e + combining acute = é (single grapheme)
        let e_acute = "e\u{0301}"; // é as e + combining acute
        // The grapheme 'é' is alphabetic, space is not, so it splits after 'é'
        let text1 = format!("{e_acute} test");
        assert_eq!(split_first_chunk(&text1), (e_acute, " test"));

        // Multiple combining marks - ensure they all stay together
        let e_with_marks = "e\u{0301}\u{0308}"; // e + acute + diaeresis
        let text2 = format!("{e_with_marks} word");
        assert_eq!(split_first_chunk(&text2), (e_with_marks, " word"));

        // Test that combining marks don't get split from base char even in longer strings
        let text = format!("te{e_acute}st");
        let result = split_first_chunk(&text);
        // The entire word is alphabetic, so it stays together
        assert_eq!(result, (text.as_str(), ""));
        // Important: verify the grapheme is intact (not split between e and combining mark)
        assert!(result.0.contains("\u{0301}"));
    }

    #[test]
    fn test_split_first_chunk_flag_emoji() {
        // Flag emojis are composed of two regional indicator symbols
        // 🇺🇸 = U+1F1FA (regional indicator U) + U+1F1F8 (regional indicator S)
        assert_eq!(split_first_chunk("🇺🇸America"), ("🇺🇸", "America"));
        assert_eq!(split_first_chunk("🇺🇸 test"), ("🇺🇸 ", "test"));
    }

    #[test]
    fn test_split_first_chunk_emoji_zwj_sequences() {
        // Zero-width joiner (ZWJ) sequences should not be split
        // Woman technologist: 👩 + ZWJ + 💻
        assert_eq!(split_first_chunk("👩‍💻code"), ("👩‍💻", "code"));

        // Rainbow flag: 🏳 + ZWJ + 🌈
        assert_eq!(split_first_chunk("🏳️‍🌈pride"), ("🏳️‍🌈", "pride"));
    }

    #[test]
    fn test_split_first_chunk_keycap_sequences() {
        // Keycap sequences: digit/symbol + variation selector + combining enclosing keycap
        // Example: 1️⃣ = '1' + U+FE0F (variation selector) + U+20E3 (combining enclosing keycap)
        assert_eq!(split_first_chunk("1️⃣test"), ("1️⃣", "test"));
        assert_eq!(split_first_chunk("*️⃣word"), ("*️⃣", "word"));
    }

    #[test]
    fn test_split_first_chunk_multiple_grapheme_edge_cases() {
        // Mix of different complex graphemes in sequence
        assert_eq!(split_first_chunk("👍🏻👨‍👩‍👧‍👦🇺🇸test"), ("👍🏻👨‍👩‍👧‍👦🇺🇸", "test"));

        // Text with accented characters
        let text = "café"; // é is a precomposed character
        assert_eq!(split_first_chunk(text), (text, ""));

        // Combining marks in the middle of a word
        let text_combining = "cafe\u{0301}"; // café with combining acute
        assert_eq!(split_first_chunk(text_combining), (text_combining, ""));
    }

    #[test]
    fn test_extract_first_n_width_basic() {
        // Basic ASCII text
        assert_eq!(extract_first_n_width("hello world", 5), ("hello", " world"));
        assert_eq!(extract_first_n_width("hello", 5), ("hello", ""));
        assert_eq!(extract_first_n_width("hello", 10), ("hello", ""));
        assert_eq!(extract_first_n_width("hello world", 0), ("", "hello world"));
    }

    #[test]
    fn test_extract_first_n_width_exact_fit() {
        // Text that fits exactly
        assert_eq!(extract_first_n_width("test", 4), ("test", ""));
        assert_eq!(extract_first_n_width("12345", 5), ("12345", ""));
    }

    #[test]
    fn test_extract_first_n_width_emojis() {
        // Most emojis have width 2
        assert_eq!(extract_first_n_width("👍test", 2), ("👍", "test"));
        assert_eq!(extract_first_n_width("👍test", 3), ("👍t", "est"));
        assert_eq!(extract_first_n_width("👍test", 6), ("👍test", ""));

        // Multiple emojis
        assert_eq!(extract_first_n_width("👍👎🎉", 2), ("👍", "👎🎉"));
        assert_eq!(extract_first_n_width("👍👎🎉", 4), ("👍👎", "🎉"));
        assert_eq!(extract_first_n_width("👍👎🎉", 6), ("👍👎🎉", ""));

        // Emoji with text
        assert_eq!(extract_first_n_width("✅ PASSED", 2), ("✅", " PASSED"));
        assert_eq!(extract_first_n_width("✅ PASSED", 3), ("✅ ", "PASSED"));
        assert_eq!(extract_first_n_width("PASSED ✅", 6), ("PASSED", " ✅"));
    }

    #[test]
    fn test_extract_first_n_width_wide_characters() {
        // Chinese/Japanese characters typically have width 2
        assert_eq!(extract_first_n_width("日本語", 2), ("日", "本語"));
        assert_eq!(extract_first_n_width("日本語", 4), ("日本", "語"));
        assert_eq!(extract_first_n_width("日本語", 6), ("日本語", ""));

        // Mixed ASCII and wide characters
        assert_eq!(extract_first_n_width("test日本", 4), ("test", "日本"));
        assert_eq!(extract_first_n_width("test日本", 6), ("test日", "本"));
    }

    #[test]
    fn test_extract_first_n_width_empty() {
        // Empty string
        assert_eq!(extract_first_n_width("", 0), ("", ""));
        assert_eq!(extract_first_n_width("", 10), ("", ""));
    }

    #[test]
    fn test_extract_first_n_width_zero_width_chars() {
        // Some combining characters have width 0
        // The combining diaeresis (¨) has width 0 when combined
        let text = "e\u{0308}"; // ë (e + combining diaeresis)
        assert_eq!(extract_first_n_width(text, 1), (text, ""));
    }

    #[test]
    fn test_extract_first_n_width_complex_emojis() {
        // Some emojis are composed of multiple code points, e.g. the family emoji (👨‍👩‍👧‍👦) is composed using zero-width joiners.
        // (Note that this emoji will often not render properly in some terminals etc.)
        // With grapheme clusters, these are kept together as a single unit.
        assert_eq!(extract_first_n_width("👨‍👩‍👧‍👦test", 2), ("👨‍👩‍👧‍👦", "test"));
        assert_eq!(extract_first_n_width("👨‍👩‍👧‍👦test", 4), ("👨‍👩‍👧‍👦te", "st"));

        // Skin tone modifiers - kept as a single grapheme
        assert_eq!(extract_first_n_width("👍🏻test", 2), ("👍🏻", "test"));
        assert_eq!(extract_first_n_width("👍🏻test", 4), ("👍🏻te", "st"));
    }

    #[test]
    fn test_extract_first_n_width_boundary_conditions() {
        // Test splitting exactly at character boundary
        assert_eq!(extract_first_n_width("abc", 2), ("ab", "c"));
        assert_eq!(extract_first_n_width("abc", 1), ("a", "bc"));

        // Test with max_width larger than string
        assert_eq!(extract_first_n_width("short", 100), ("short", ""));
    }

    #[test]
    fn test_extract_first_n_width_mixed_content() {
        // Mix of ASCII, wide chars, and emojis
        assert_eq!(extract_first_n_width("test🎉日本", 4), ("test", "🎉日本"));
        assert_eq!(extract_first_n_width("test🎉日本", 6), ("test🎉", "日本"));
        assert_eq!(extract_first_n_width("test🎉日本", 8), ("test🎉日", "本"));
    }

    #[test]
    fn test_extract_first_n_width_whitespace() {
        // Spaces have width 1
        assert_eq!(extract_first_n_width("   test", 2), ("  ", " test"));
        assert_eq!(extract_first_n_width("   test", 3), ("   ", "test"));

        // Tabs have varying width, but typically treated as 0 or 1 by unicode_width
        assert_eq!(extract_first_n_width("\ttest", 0), ("", "\ttest"));
    }

    mod wrap_lines_tests {
        use super::*;

        // Helper function to create a simple unstyled line
        fn line(text: &str) -> StyledLine {
            vec![(Cow::Owned(text.to_string()), None)]
        }

        // Helper function to create a styled line
        fn styled_line(segments: Vec<(&str, Option<Style>)>) -> StyledLine {
            segments
                .into_iter()
                .map(|(text, style)| (Cow::Owned(text.to_string()), style))
                .collect()
        }

        // Helper to convert result to vec of strings (ignoring styles) for easier assertions
        fn to_strings(lines: Vec<StyledLine>) -> Vec<String> {
            lines
                .into_iter()
                .map(|line| {
                    line.into_iter()
                        .map(|(text, _)| text.to_string())
                        .collect::<String>()
                })
                .collect()
        }

        #[test]
        fn test_no_wrapping_needed() {
            let input = vec![line("foo"), line("bar")];
            let result = wrap_lines(input, 10, None);

            assert_eq!(to_strings(result), vec!["foo", "bar"]);
        }

        #[test]
        fn test_basic_wrapping() {
            let input = vec![line("foo bar baz")];
            let result = wrap_lines(input, 8, None);

            assert_eq!(to_strings(result), vec!["foo bar ", "  ↪ baz"]);
        }

        #[test]
        fn test_multiple_lines_wrapping() {
            // Two input lines that both need wrapping
            let input = vec![line("foo bar baz"), line("qux quux corge")];
            let result = wrap_lines(input, 8, None);

            assert_eq!(
                to_strings(result),
                vec!["foo bar ", "  ↪ baz", "qux quux", "  ↪  cor", "  ↪ ge"]
            );
        }

        #[test]
        fn test_wrap_lines_width_zero() {
            let input = vec![line("foo")];
            let result = wrap_lines(input, 0, None);

            assert_eq!(result.len(), 0);
        }

        #[test]
        fn test_wrap_lines_width_one() {
            let input = vec![line("foo")];
            let result = wrap_lines(input, 1, None);

            // Should return empty since we can't fit meaningful content with continuation prefix
            assert_eq!(result.len(), 0);
        }

        #[test]
        fn test_wrap_lines_width_equals_prefix() {
            // Width equal to WRAPPED_LINE_PREFIX length (4)
            // Should return nothing as we can't fit continuation prefix
            let input = vec![line("foo bar")];
            let result = wrap_lines(input, 4, None);
            assert_eq!(result.len(), 0);
        }

        #[test]
        fn test_wrap_lines_width_one_more_than_prefix() {
            // Width of 5 (one more than WRAPPED_LINE_PREFIX length)
            // Should be able to wrap with 1 char per continuation line
            let input = vec![line("hello world")];
            let result = wrap_lines(input, 5, None);

            assert_eq!(
                to_strings(result),
                vec![
                    "hello", "  ↪  ", "  ↪ w", "  ↪ o", "  ↪ r", "  ↪ l", "  ↪ d"
                ]
            );
        }

        #[test]
        fn test_wrap_lines_num_lines_limit() {
            // Should stop after num_lines
            let input = vec![line("foo bar baz qux")];
            let result = wrap_lines(input, 8, Some(2));

            assert_eq!(to_strings(result), vec!["foo bar ", "  ↪ baz "]);
        }

        #[test]
        fn test_wrap_lines_num_lines_zero() {
            // num_lines of 0 should return empty
            let input = vec![line("foo")];
            let result = wrap_lines(input, 10, Some(0));

            assert_eq!(result.len(), 0);
        }

        #[test]
        fn test_wrap_lines_empty_input() {
            let input: Vec<StyledLine> = vec![];
            let result = wrap_lines(input, 10, None);

            assert_eq!(result.len(), 0);
        }

        #[test]
        fn test_wrap_lines_single_space() {
            let input = vec![line(" ")];
            let result = wrap_lines(input, 10, None);

            assert_eq!(result, vec![line(" ")]);
        }

        #[test]
        fn test_very_long_word() {
            // A single word longer than width should be broken up
            let input = vec![line("verylongword")];
            let result = wrap_lines(input, 8, None);

            assert_eq!(to_strings(result), vec!["verylong", "  ↪ word"]);
        }

        #[test]
        fn test_exact_width_fit() {
            // Text that exactly fits the width
            let input = vec![line("12345678")];
            let result = wrap_lines(input, 8, None);

            assert_eq!(to_strings(result), vec!["12345678"]);
        }

        #[test]
        fn test_one_char_over_width() {
            let input = vec![line("123456789")];
            let result = wrap_lines(input, 8, None);

            assert_eq!(to_strings(result), vec!["12345678", "  ↪ 9"]);
        }

        #[test]
        fn test_styled_segments_preserved() {
            // Styles should be preserved through wrapping
            let bold_style = Some(Style::default().bold());
            let input = vec![styled_line(vec![
                ("foo ", None),
                ("bar baz", bold_style),
                (" qux", None),
            ])];

            let result = wrap_lines(input, 6, None);

            let dim_style = Some(Style::default().dim());

            assert_eq!(
                result,
                vec![
                    styled_line(vec![("foo", None), (" ", None), ("ba", bold_style)]),
                    styled_line(vec![
                        ("  ↪ ", dim_style),
                        ("r", bold_style),
                        (" ", bold_style)
                    ]),
                    styled_line(vec![("  ↪ ", dim_style), ("ba", bold_style)]),
                    styled_line(vec![("  ↪ ", dim_style), ("z", bold_style), (" ", None)]),
                    styled_line(vec![("  ↪ ", dim_style), ("qu", None)]),
                    styled_line(vec![("  ↪ ", dim_style), ("x", None)]),
                ]
            );
        }

        #[test]
        fn test_unicode_emoji() {
            // Emojis should not be split
            // "Hello 👨‍👩‍👧‍👦 World" - family emoji is width 2
            let input = vec![line("Hello 👨‍👩‍👧‍👦 World")];
            let result = wrap_lines(input, 12, None);

            // "Hello 👨‍👩‍👧‍👦 " = 5 + 1 + 2 + 1 = 9 cols, "World" = 5 cols
            // Should wrap: "Hello 👨‍👩‍👧‍👦 " + "  ↪ World"
            let strings = to_strings(result);
            assert_eq!(strings, vec!["Hello 👨‍👩‍👧‍👦 ", "  ↪ World"]);
        }

        #[test]
        fn test_wide_characters_wrapping() {
            // CJK characters that need wrapping
            // "你好世界朋友" = 6 chars = 12 columns, width = 10
            let input = vec![line("你好世界朋友")];
            let result = wrap_lines(input, 10, None);

            assert_eq!(to_strings(result), vec!["你好世界朋", "  ↪ 友"]);
        }

        #[test]
        fn test_mixed_narrow_wide_characters() {
            // "Hello 世界" = 5 ASCII + 1 space + 2 CJK (4 cols) = 10 cols
            let input = vec![line("Hello 世界")];
            let result = wrap_lines(input, 10, None);

            // Should fit exactly
            assert_eq!(to_strings(result), vec!["Hello 世界"]);
        }

        #[test]
        fn test_wrap_lines_multiple_input_lines_with_limit() {
            // Multiple input lines with num_lines limit
            let input = vec![line("foo bar"), line("baz qux")];
            let result = wrap_lines(input, 6, Some(3));

            assert_eq!(to_strings(result), vec!["foo ba", "  ↪ r", "baz qu"]);
        }

        #[test]
        fn test_wrap_lines_multiple_spaces() {
            let input = vec![line("     ")];
            let result = wrap_lines(input, 10, None);

            assert_eq!(to_strings(result), vec!["     "]);
        }

        #[test]
        fn test_wrap_lines_multiple_wraps_same_line() {
            // A line that needs to wrap multiple times
            let input = vec![line("one two three four five six")];
            let result = wrap_lines(input, 8, None);

            // Due to chunking, wrapping happens differently than word boundaries
            assert_eq!(
                to_strings(result),
                vec![
                    "one two ",
                    "  ↪ thre",
                    "  ↪ e ",
                    "  ↪ four",
                    "  ↪  ",
                    "  ↪ five",
                    "  ↪  six"
                ]
            );
        }
    }
}
