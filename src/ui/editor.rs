use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

const MATH_COLOR: Color = Color::Cyan;

use crate::app::{App, EditorView, Modal, RefineStatus};
use crate::dictation::DictationStatus;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_divider(f, app, chunks[1]);
    draw_body(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let (title, created, modified) = match &app.editing {
        Some(n) => (
            n.meta.title.to_uppercase(),
            fmt_local(&n.meta.created),
            fmt_local(&n.meta.modified),
        ),
        None => (String::new(), String::new(), String::new()),
    };

    let title_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .bg(theme.title_bg)
        .fg(theme.title_fg);

    // A ✦ marker (in its own configurable color) precedes the title in the
    // refined view; the original view shows just the title.
    let line1 = if app.editor_view == EditorView::Refined {
        Line::from(vec![
            Span::styled(
                " ✦ ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .bg(theme.title_bg)
                    .fg(Color::Rgb(127, 0, 255)),
            ),
            Span::styled(format!("{title} "), title_style),
        ])
    } else {
        Line::from(Span::styled(format!(" {title} "), title_style))
    };

    let line2 = Line::from(Span::styled(
        format!(" created {created}    modified {modified}"),
        Style::default().fg(theme.meta),
    ));
    f.render_widget(Paragraph::new(vec![line1, line2]), area);
}

fn draw_divider(f: &mut Frame, app: &App, area: Rect) {
    let line = "─".repeat(area.width as usize);
    let p = Paragraph::new(Line::from(Span::styled(
        line,
        Style::default().fg(app.theme.divider),
    )));
    f.render_widget(p, area);
}

/// Render the editable note body with word-wrapping at the current window width.
///
/// `tui-textarea` only holds the text/cursor model (it can't soft-wrap), so we
/// wrap its logical lines to the inner width here, scroll to keep the cursor
/// visible, and place the terminal cursor at the wrapped position. The stored
/// content is never modified by wrapping.
fn draw_body(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let border = match app.editor_view {
        EditorView::Original => theme.border,
        EditorView::Refined => theme.refined,
    };
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border))
        .padding(Padding::new(theme.padding, theme.padding, 1, 1));
    let inner = block.inner(area);
    let width = (inner.width as usize).max(1);
    let height = inner.height as usize;

    let (cursor_row, cursor_col) = app.textarea.cursor();
    let wrapped = wrap_lines(app.textarea.lines(), cursor_row, cursor_col, width);

    // Vertical scroll so the cursor row stays on screen.
    let scroll = if height > 0 && wrapped.cursor_row >= height {
        wrapped.cursor_row - height + 1
    } else {
        0
    };

    let text = Text::from(style_math_rows(&wrapped.rows));
    let para = Paragraph::new(text).block(block).scroll((scroll as u16, 0));
    f.render_widget(para, area);

    // Place the cursor at the wrapped position (clamped on screen), but only when
    // the editor actually owns input. With a modal layered on top the cursor would
    // otherwise float as a stray block over the modal.
    if height > 0 && matches!(app.modal, Modal::None) {
        let crow = wrapped.cursor_row.saturating_sub(scroll) as u16;
        let max_col = inner.width.saturating_sub(1);
        let cx = inner.x + (wrapped.cursor_col as u16).min(max_col);
        let cy = inner.y + crow;
        match voice_meter(app) {
            // While listening, the terminal can't scale or colorize its native
            // cursor, so paint the cursor cell as a live input-level block instead.
            // Leaving the native cursor unset hides it for this frame.
            Some((glyph, color)) => {
                let cell = Rect::new(cx, cy, 1, 1);
                let block = Paragraph::new(Line::from(Span::styled(
                    glyph.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )));
                f.render_widget(block, cell);
            }
            None => f.set_cursor_position((cx, cy)),
        }
    }
}

/// Partial-block glyphs filling a cell from the bottom, 1/8 up to full height.
const METER_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// While dictation is capturing, turn the cursor into a live mic-level meter: a
/// block whose height (via [`METER_BLOCKS`]) and color (green → red) track the
/// input level. Returns the glyph + color to paint, or `None` when not listening.
fn voice_meter(app: &App) -> Option<(char, Color)> {
    if !matches!(
        app.dictation_status(),
        DictationStatus::Listening | DictationStatus::Live
    ) {
        return None;
    }
    let level = app.dictation_level().clamp(0.0, 1.0);
    Some((level_block(level), level_color(level)))
}

/// Pick the block glyph for `level` in [0, 1]. Always at least the shortest block
/// so the cursor stays visible (the resting "listening" state) at silence.
fn level_block(level: f32) -> char {
    let n = METER_BLOCKS.len();
    let idx = ((level * n as f32).ceil() as usize).clamp(1, n);
    METER_BLOCKS[idx - 1]
}

/// Interpolate the meter color from green (quiet) through to red (loud).
fn level_color(level: f32) -> Color {
    let t = level.clamp(0.0, 1.0);
    Color::Rgb((t * 255.0) as u8, ((1.0 - t) * 255.0) as u8, 0)
}

/// A logical buffer wrapped into display rows, with the cursor mapped into it.
struct Wrapped {
    rows: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

/// Wrap `lines` to `width` columns, breaking on spaces where possible, and map
/// the logical cursor at `(cur_row, cur_col)` to its display row/column.
fn wrap_lines(lines: &[String], cur_row: usize, cur_col: usize, width: usize) -> Wrapped {
    let width = width.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut cursor_row = 0;
    let mut cursor_col = 0;

    for (li, line) in lines.iter().enumerate() {
        let base = rows.len();
        let segs = wrap_one(line, width);
        if li == cur_row {
            let (dr, dc) = locate(&segs, cur_col);
            cursor_row = base + dr;
            cursor_col = dc;
        }
        let chars: Vec<char> = line.chars().collect();
        for &(start, len) in &segs {
            rows.push(chars[start..start + len].iter().collect());
        }
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    Wrapped {
        rows,
        cursor_row,
        cursor_col,
    }
}

/// Split one logical line into `(start, len)` character segments no wider than
/// `width`, preferring to break after a space. Every character is covered once.
fn wrap_one(line: &str, width: usize) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    if n == 0 {
        return vec![(0, 0)];
    }
    let mut segs = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = (start + width).min(n);
        if end < n {
            if let Some(sp) = (start..end).rev().find(|&i| chars[i] == ' ') {
                // Break just after the space (keeps it on the current row).
                end = sp + 1;
            }
        }
        if end == start {
            end = (start + width).min(n); // a word longer than the width
        }
        segs.push((start, end - start));
        start = end;
    }
    segs
}

/// Map a character column to its `(segment_index, offset)` within `segs`.
fn locate(segs: &[(usize, usize)], col: usize) -> (usize, usize) {
    let last = segs.len().saturating_sub(1);
    for (i, &(start, len)) in segs.iter().enumerate() {
        if col < start + len || i == last {
            return (i, col - start);
        }
    }
    (0, 0)
}

/// Whether the parser is inside a math region that opened on a previous row.
#[derive(Clone, Copy)]
enum MathState {
    Normal,
    InInline,  // inside $...$  that did not close on its opening row
    InDisplay, // inside $$...$$ that did not close on its opening row
}

/// Style all wrapped rows together, threading [`MathState`] across them so a
/// `$$...$$` block that wraps to a second (or third …) row stays fully cyan.
fn style_math_rows(rows: &[String]) -> Vec<Line<'static>> {
    let mut state = MathState::Normal;
    rows.iter()
        .map(|s| {
            let (line, next) = style_math_row(s, state);
            state = next;
            line
        })
        .collect()
}

/// Style one wrapped row, starting from `state` and returning the row's [`Line`]
/// plus the [`MathState`] to carry into the next row.
fn style_math_row(s: &str, state: MathState) -> (Line<'static>, MathState) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut pos = 0;
    let mut st = state;
    let math_style = Style::default().fg(MATH_COLOR);

    while pos < s.len() {
        match st {
            MathState::Normal => match s[pos..].find('$') {
                None => {
                    spans.push(Span::raw(s[pos..].to_string()));
                    pos = s.len();
                }
                Some(rel) => {
                    let dollar = pos + rel;
                    if dollar > pos {
                        spans.push(Span::raw(s[pos..dollar].to_string()));
                    }
                    if s[dollar..].starts_with("$$") {
                        let from = dollar + 2;
                        match s[from..].find("$$") {
                            Some(rc) => {
                                let end = from + rc + 2;
                                spans.push(Span::styled(s[dollar..end].to_string(), math_style));
                                pos = end;
                            }
                            None => {
                                spans.push(Span::styled(s[dollar..].to_string(), math_style));
                                pos = s.len();
                                st = MathState::InDisplay;
                            }
                        }
                    } else {
                        let from = dollar + 1;
                        match s[from..].find('$') {
                            Some(rc) => {
                                let end = from + rc + 1;
                                spans.push(Span::styled(s[dollar..end].to_string(), math_style));
                                pos = end;
                            }
                            None => {
                                spans.push(Span::styled(s[dollar..].to_string(), math_style));
                                pos = s.len();
                                st = MathState::InInline;
                            }
                        }
                    }
                }
            },
            MathState::InDisplay => match s[pos..].find("$$") {
                Some(rel) => {
                    let end = pos + rel + 2;
                    spans.push(Span::styled(s[pos..end].to_string(), math_style));
                    pos = end;
                    st = MathState::Normal;
                }
                None => {
                    spans.push(Span::styled(s[pos..].to_string(), math_style));
                    pos = s.len();
                }
            },
            MathState::InInline => match s[pos..].find('$') {
                Some(rel) => {
                    let end = pos + rel + 1;
                    spans.push(Span::styled(s[pos..end].to_string(), math_style));
                    pos = end;
                    st = MathState::Normal;
                }
                None => {
                    spans.push(Span::styled(s[pos..].to_string(), math_style));
                    pos = s.len();
                }
            },
        }
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    (Line::from(spans), st)
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let hint = app
        .cfg
        .show_shortcuts
        .then_some(" ^R refine · ^T title · ^H help ");

    // Right indicator: a red, animated "listening" cue (mirroring the refine
    // spinner) takes priority over the refine status while dictation is active.
    let right = match app.dictation_status() {
        DictationStatus::Listening | DictationStatus::Live => {
            Some((format!("{} ● LISTENING", app.spinner()), Color::Red))
        }
        DictationStatus::Transcribing => {
            Some((format!("{} Transcribing", app.spinner()), Color::Red))
        }
        _ => match app.refine_status() {
            RefineStatus::Idle => None,
            RefineStatus::Refining => {
                Some((format!("{} Refining", app.spinner()), app.theme.accent))
            }
            RefineStatus::Done => Some(("Done".to_string(), app.theme.status)),
            RefineStatus::Error => Some(("Error".to_string(), Color::Red)),
        },
    };
    let right = right.as_ref().map(|(t, c)| (t.as_str(), *c));

    // Left: a transient message (the "Dictation ready" flash, dictation errors,
    // save notices) wins; while the model loads/downloads, show that; otherwise
    // the hint.
    let left = if !app.status.is_empty() {
        app.status.clone()
    } else if app.dictation_status() == DictationStatus::Preparing {
        format!("{} Preparing dictation for first use…", app.spinner())
    } else if app.dictation_status() == DictationStatus::Loading {
        "⏳ Loading speech model…".to_string()
    } else {
        String::new()
    };

    crate::ui::draw_bottom_bar(f, area, &app.theme, hint, &left, right);
}

fn fmt_local(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.with_timezone(&chrono::Local)
        .format("%b %-d %Y  %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str], width: usize) -> Vec<String> {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        wrap_lines(&owned, 0, 0, width).rows
    }

    #[test]
    fn wraps_long_line_on_spaces() {
        let r = rows(&["the quick brown fox jumps"], 10);
        // Each row fits within the width and breaks at spaces.
        assert!(r.iter().all(|l| l.chars().count() <= 10));
        assert_eq!(r.concat().replace(' ', ""), "thequickbrownfoxjumps");
        assert!(r.len() >= 3);
    }

    #[test]
    fn hard_breaks_word_longer_than_width() {
        let r = rows(&["supercalifragilistic"], 5);
        assert!(r.iter().all(|l| l.chars().count() <= 5));
        assert_eq!(r.concat(), "supercalifragilistic");
    }

    #[test]
    fn preserves_blank_lines_and_maps_cursor() {
        let owned = vec!["hello world".to_string(), String::new(), "end".to_string()];
        // Cursor at end of "hello world" (col 11) with width 6 -> second wrapped row.
        let w = wrap_lines(&owned, 0, 11, 6);
        assert_eq!(w.rows, vec!["hello ", "world", "", "end"]);
        assert_eq!(w.cursor_row, 1);
        assert_eq!(w.cursor_col, 5);
    }

    #[test]
    fn meter_block_grows_with_level_and_never_vanishes() {
        // Silence still shows the shortest block (the cursor stays visible).
        assert_eq!(level_block(0.0), '▁');
        // Full level is the full block; the glyph rises monotonically in between.
        assert_eq!(level_block(1.0), '█');
        assert!(level_block(0.2) < level_block(0.8)); // chars order by height
    }

    #[test]
    fn meter_color_runs_green_to_red() {
        assert_eq!(level_color(0.0), Color::Rgb(0, 255, 0)); // quiet -> green
        assert_eq!(level_color(1.0), Color::Rgb(255, 0, 0)); // loud  -> red
    }

    fn spans_of(line: Line) -> Vec<(String, bool)> {
        line.spans
            .into_iter()
            .map(|s| (s.content.to_string(), s.style.fg == Some(MATH_COLOR)))
            .collect()
    }

    #[test]
    fn inline_math_highlighted_on_one_row() {
        let (line, st) = style_math_row("area = $pi r^2$ done", MathState::Normal);
        assert!(matches!(st, MathState::Normal));
        let s = spans_of(line);
        assert_eq!(s[0], ("area = ".into(), false));
        assert_eq!(s[1], ("$pi r^2$".into(), true));
        assert_eq!(s[2], (" done".into(), false));
    }

    #[test]
    fn display_math_carries_cyan_across_wrapped_rows() {
        // Simulate a $$...$$ that word-wraps: the opening is on row 0, the rest
        // on row 1. Both rows must be fully cyan.
        let row0 = "$$E = mc^2 +".to_string();
        let row1 = "some term$$".to_string();
        let lines = style_math_rows(&[row0, row1]);
        let r0 = spans_of(lines[0].clone());
        let r1 = spans_of(lines[1].clone());
        assert!(r0.iter().all(|(_, cyan)| *cyan), "row 0 must be fully cyan");
        assert!(r1.iter().all(|(_, cyan)| *cyan), "row 1 must be fully cyan");
    }

    #[test]
    fn plain_text_after_closed_display_block_is_not_cyan() {
        let rows = vec!["$$a$$ plain".to_string()];
        let lines = style_math_rows(&rows);
        let s = spans_of(lines[0].clone());
        assert_eq!(s[0], ("$$a$$".into(), true));
        assert_eq!(s[1], (" plain".into(), false));
    }
}
