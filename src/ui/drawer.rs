use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use crate::app::{App, DrawerRow, Focus};
use crate::ui::truncate;

/// Width of the drawer pane, including its right-hand divider border.
pub const DRAWER_W: u16 = 30;

/// Light grey for the drawer's note titles and labels — a softer shade than the
/// tile borders so the tree reads as secondary chrome.
const TEXT: Color = Color::Gray;

/// Draw the notes/folders tree in `area`. Mutates `app.drawer_scroll` so the
/// cursor row stays visible — the drawer scrolls independently of the main
/// screen. Highlights the cursor row when the drawer holds focus, and marks the
/// note currently open in the editor.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    // Copy out the colours we need so no borrow of `app.theme` is held while we
    // later mutate `app.drawer_scroll`.
    let accent = app.theme.accent;
    let border = app.theme.border;
    let folder_color = app.theme.refined;
    let star = ratatui::style::Color::Rgb(127, 0, 255);

    let focused = matches!(app.focus, Focus::Drawer);
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(if focused { accent } else { border }))
        // A little more breathing room on the right, before the divider.
        .padding(Padding::new(1, 2, 0, 0));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let inner_w = inner.width as usize;

    let total = app.notes.len();
    let count_str = format!("{total} note{}", if total == 1 { "" } else { "s" });

    let header = Line::from(Span::styled(
        truncate("NOTES & FOLDERS", inner_w),
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
    ));
    let subheader = Line::from(Span::styled(
        truncate(&count_str, inner_w),
        Style::default().fg(border),
    ));

    let rows = app.drawer_rows();
    if rows.is_empty() {
        let text = Text::from(vec![
            header,
            subheader,
            Line::raw(""),
            Line::from(Span::styled("  (no notes yet)", Style::default().fg(TEXT))),
        ]);
        f.render_widget(Paragraph::new(text), inner);
        return;
    }

    // Two lines are taken by the header + note count; the rest is the scrollable row viewport.
    let view_h = inner.height.saturating_sub(2) as usize;
    let sel = app.drawer_selected.min(rows.len() - 1);
    let scroll = clamp_scroll(app.drawer_scroll, sel, rows.len(), view_h);
    app.drawer_scroll = scroll;

    let current_note = app.editing.as_ref().map(|n| n.meta.id.clone());

    let mut lines = Vec::with_capacity(view_h + 2);
    lines.push(header);
    lines.push(subheader);
    for (i, row) in rows.iter().enumerate().skip(scroll).take(view_h) {
        let selected = focused && i == sel;
        lines.push(render_row(
            app,
            *row,
            selected,
            current_note.as_deref(),
            inner_w,
            (accent, folder_color, star),
        ));
    }

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

/// Pick a scroll offset that keeps `sel` within a `view_h`-tall window.
fn clamp_scroll(mut scroll: usize, sel: usize, len: usize, view_h: usize) -> usize {
    if view_h == 0 {
        return 0;
    }
    if sel < scroll {
        scroll = sel;
    } else if sel >= scroll + view_h {
        scroll = sel + 1 - view_h;
    }
    scroll.min(len.saturating_sub(view_h))
}

fn render_row(
    app: &App,
    row: DrawerRow,
    selected: bool,
    current_note: Option<&str>,
    width: usize,
    colors: (Color, Color, Color),
) -> Line<'static> {
    let (accent, folder_color, star) = colors;
    let marker = if selected { "› " } else { "  " };

    match row {
        DrawerRow::Folder {
            index,
            expanded,
            count,
        } => {
            let folder = &app.folders[index];
            let arrow = if expanded { "▾" } else { "▸" };
            let label = format!("{marker}{arrow} {} ({count})", folder.title);
            let style = if selected {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(folder_color)
                    .add_modifier(Modifier::BOLD)
            };
            Line::from(Span::styled(truncate(&label, width), style))
        }
        DrawerRow::Note { index, child } => {
            let note = &app.notes[index];
            let indent = if child { "  " } else { "" };
            let is_current = current_note == Some(note.meta.id.as_str());
            let label = format!("{marker}{indent}• {}", note.meta.title);
            let mut style = if selected {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default().fg(accent)
            } else {
                Style::default().fg(TEXT)
            };
            if is_current {
                style = style.add_modifier(Modifier::BOLD);
            }
            // Reserve room for a trailing refined marker so it isn't truncated off.
            let refined = note.refined.is_some();
            let body_w = if refined {
                width.saturating_sub(2)
            } else {
                width
            };
            let mut spans = vec![Span::styled(truncate(&label, body_w), style)];
            if refined {
                spans.push(Span::styled(" ✦", Style::default().fg(star)));
            }
            Line::from(spans)
        }
    }
}
