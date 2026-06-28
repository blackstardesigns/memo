use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, ListItem};
use crate::note::{Folder, Note};
use crate::theme::ResolvedTheme;
use crate::ui::truncate;

const TILE_W: u16 = 32;
const TILE_H: u16 = 6;
/// Gaps between tiles (and between tiles and the screen edge), in character
/// cells. Equal cell counts in both axes; note that because terminal cells are
/// taller than they are wide, the vertical gap looks a little larger on screen.
const TILE_GAP_X: u16 = 1;
const TILE_GAP_Y: u16 = 0;

/// Glyph prefixed to a folder's title so folders read differently from notes.
const FOLDER_GLYPH: &str = "▤";

/// Left inset for the home screen while the drawer is open, so its content isn't
/// flush against the drawer's divider.
const DRAWER_GAP: u16 = 2;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let area = if app.drawer_open {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(DRAWER_GAP), Constraint::Min(0)])
            .split(area)[1]
    } else {
        area
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_search(f, app, chunks[0]);
    draw_tiles(f, app, chunks[1]);
    draw_footer(f, app, chunks[2]);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let focused = matches!(app.focus, Focus::Search);
    let border_color = if focused { theme.accent } else { theme.border };
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border_color));

    let mut query = app.search_query.clone();
    if focused {
        query.push('▏');
    }
    let breadcrumb = app
        .current_folder
        .as_deref()
        .and_then(|id| app.folders.iter().find(|f| f.id == id))
        .map(|f| f.title.clone());
    let input_span = if !focused && app.search_query.is_empty() {
        match &breadcrumb {
            Some(title) => Span::styled(
                format!("  ‹ {title}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            None => Span::styled("  search…", Style::default().fg(theme.border)),
        }
    } else {
        Span::raw(format!("  {query}"))
    };
    f.render_widget(Paragraph::new(Line::from(input_span)).block(block), area);
}

fn draw_tiles(f: &mut Frame, app: &mut App, area: Rect) {
    if app.items.is_empty() {
        draw_empty(f, app, area);
        return;
    }

    // Column count uses TILE_W as the minimum tile width.
    let cols = ((area.width + TILE_GAP_X) / (TILE_W + TILE_GAP_X)).max(1) as usize;
    app.list_columns = cols;
    let rows_fit = ((area.height + TILE_GAP_Y) / (TILE_H + TILE_GAP_Y)).max(1) as usize;
    let per_page = (cols * rows_fit).max(1);
    let page = app.selected / per_page;
    let start = page * per_page;
    let end = (start + per_page).min(app.items.len());

    // Tiles expand to fill the full width. Distribute the integer-division
    // remainder equally as extra padding on the left and right so both margins
    // stay the same size.
    let tile_w = area.width.saturating_sub((cols as u16 + 1) * TILE_GAP_X) / cols as u16;
    let h_rem = area
        .width
        .saturating_sub((cols as u16 + 1) * TILE_GAP_X + cols as u16 * tile_w);
    let left_pad = TILE_GAP_X + h_rem / 2;
    let right_pad = TILE_GAP_X + h_rem - h_rem / 2;

    let mut row_constraints: Vec<Constraint> = Vec::with_capacity(2 * rows_fit + 2);
    for _ in 0..rows_fit {
        row_constraints.push(Constraint::Length(TILE_GAP_Y));
        row_constraints.push(Constraint::Length(TILE_H));
    }
    row_constraints.push(Constraint::Length(TILE_GAP_Y));
    row_constraints.push(Constraint::Fill(1));
    let all_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);
    let rows: Vec<_> = (0..rows_fit).map(|i| all_rows[2 * i + 1]).collect();

    let mut idx = start;
    'outer: for row in &rows {
        // Layout: [left_pad, tile, GAP, tile, GAP, …, tile, right_pad]
        let mut col_constraints: Vec<Constraint> = Vec::with_capacity(2 * cols + 1);
        col_constraints.push(Constraint::Length(left_pad));
        for i in 0..cols {
            col_constraints.push(Constraint::Length(tile_w));
            if i + 1 < cols {
                col_constraints.push(Constraint::Length(TILE_GAP_X));
            }
        }
        col_constraints.push(Constraint::Length(right_pad));
        let all_cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row);
        // Tile cells land at odd indices: 1, 3, 5, …
        let cells: Vec<_> = (0..cols).map(|i| all_cells[2 * i + 1]).collect();

        for cell in &cells {
            if idx >= end {
                break 'outer;
            }
            let selected = idx == app.selected;
            match app.items[idx] {
                ListItem::Folder(i) => {
                    let folder = &app.folders[i];
                    let notes = app.folder_notes(&folder.id);
                    draw_folder_tile(f, &app.theme, folder, &notes, *cell, selected);
                }
                ListItem::Note(i) => {
                    draw_tile(f, &app.theme, &app.notes[i], *cell, selected);
                }
            }
            idx += 1;
        }
    }
}

fn draw_empty(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_columns = 1;
    let searching = !app.search_query.trim().is_empty();
    let (line1, line2) = if searching {
        ("No matches.", "")
    } else if app.current_folder.is_some() {
        ("This folder is empty.", "Press  n  to add a note.")
    } else {
        (
            "No notes yet.",
            "Press  n  for a note,  Ctrl+F  for a folder.",
        )
    };
    // Vertically center the message
    let msg_h = if line2.is_empty() { 1_u16 } else { 3 };
    let top = area.height.saturating_sub(msg_h) / 2;
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top),
            Constraint::Length(msg_h),
            Constraint::Min(0),
        ])
        .split(area);
    let text = if line2.is_empty() {
        Text::from(Line::from(Span::styled(
            line1,
            Style::default().fg(app.theme.border),
        )))
    } else {
        Text::from(vec![
            Line::from(Span::styled(line1, Style::default().fg(app.theme.border))),
            Line::raw(""),
            Line::from(Span::styled(line2, Style::default().fg(app.theme.border))),
        ])
    };
    f.render_widget(Paragraph::new(text).alignment(Alignment::Center), vert[1]);
}

fn draw_tile(f: &mut Frame, theme: &ResolvedTheme, note: &Note, area: Rect, selected: bool) {
    let (border_style, border_type) = if selected {
        let bt = if theme.rounded_tiles {
            BorderType::Rounded
        } else {
            BorderType::Thick
        };
        (
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            bt,
        )
    } else {
        let bt = if theme.rounded_tiles {
            BorderType::Rounded
        } else {
            BorderType::Plain
        };
        (Style::default().fg(theme.border), bt)
    };

    let inner_w = area.width.saturating_sub(2 + 2 * theme.padding).max(1) as usize;

    let date = note
        .meta
        .modified
        .with_timezone(&chrono::Local)
        .format("%b %-d %Y")
        .to_string();

    // Up to 2 non-empty content lines for preview; prefer refined if available
    let source = note.refined.as_deref().unwrap_or(&note.content);
    let preview: Vec<String> = source
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(2)
        .collect();

    let title_style = if selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };

    let refined = note.refined.is_some();
    // Reserve 2 chars for " ✦" so the star is always on the same line as the title.
    let title_w = if refined {
        inner_w.saturating_sub(2)
    } else {
        inner_w
    };
    let mut title_spans = vec![Span::styled(
        truncate(&note.meta.title, title_w),
        title_style,
    )];
    if refined {
        title_spans.push(Span::styled(" ✦", Style::default().fg(theme.star)));
    }

    let (p1, p2) = match preview.as_slice() {
        [] => (String::from("(empty)"), String::new()),
        [a] => (a.clone(), String::new()),
        [a, b, ..] => (a.clone(), b.clone()),
    };

    let text = Text::from(vec![
        Line::from(title_spans),
        Line::from(Span::styled(date, Style::default().fg(theme.meta))),
        Line::from(Span::styled(
            truncate(&p1, inner_w),
            Style::default().fg(theme.border),
        )),
        Line::from(Span::styled(
            truncate(&p2, inner_w),
            Style::default().fg(theme.border),
        )),
    ]);

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .padding(Padding::horizontal(theme.padding)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(para, area);
}

/// A folder tile: the folder name + note count in the border title, with a
/// vertical list of the contained note titles inside. Uses the same horizontal
/// padding as a note tile.
fn draw_folder_tile(
    f: &mut Frame,
    theme: &ResolvedTheme,
    folder: &Folder,
    notes: &[&Note],
    area: Rect,
    selected: bool,
) {
    let border_type = if theme.rounded_tiles {
        BorderType::Rounded
    } else if selected {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    // Unselected folders use the "refined" accent so they read differently from
    // notes; selected ones share the note highlight.
    let accent = if selected {
        theme.accent
    } else {
        theme.refined
    };
    let border_style = if selected {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(accent)
    };

    let n = notes.len();
    let title = format!(
        "{FOLDER_GLYPH} {} · {} note{}  ",
        folder.title,
        n,
        if n == 1 { "" } else { "s" }
    );
    let title_span = Span::styled(
        truncate(&title, area.width.saturating_sub(2) as usize),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style)
        .padding(Padding::horizontal(theme.padding))
        .title(title_span);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = if notes.is_empty() {
        vec![Line::from(Span::styled(
            "(empty)",
            Style::default().fg(theme.border),
        ))]
    } else {
        // List as many titles as fit; if they overflow, keep the last row for a
        // "+N more" summary.
        let capacity = inner.height as usize;
        let (shown, overflow) = if notes.len() <= capacity {
            (notes.len(), 0)
        } else {
            let shown = capacity.saturating_sub(1);
            (shown, notes.len() - shown)
        };
        let mut lines: Vec<Line> = notes
            .iter()
            .take(shown)
            .map(|n| {
                Line::from(Span::styled(
                    truncate(&format!("• {}", n.meta.title), inner_w),
                    Style::default().fg(Color::Gray),
                ))
            })
            .collect();
        if overflow > 0 {
            lines.push(Line::from(Span::styled(
                truncate(&format!("+{overflow} more"), inner_w),
                Style::default().fg(theme.meta),
            )));
        }
        lines
    };

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = if matches!(app.focus, Focus::Drawer) {
        " ↑↓ move · →← expand · ↵ open · Tab tiles · h/Esc close "
    } else if matches!(app.focus, Focus::Search) {
        " type to filter · ↑↓ move · Enter open · Esc or / dismiss "
    } else if app.current_folder.is_some() {
        " n new · m move · ↵ open · h drawer · Esc back · ? help "
    } else {
        " n new · ^F folder · m move · ↵ open · h drawer · ? help · q quit "
    };
    let hint = app.cfg.show_shortcuts.then_some(text);

    // Brand: star in theme star color, "memo" in a fixed muted tone.
    let brand_w = " ✦  memo ".chars().count() as u16;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(brand_w)])
        .split(area);

    crate::ui::draw_bottom_bar(f, cols[0], &app.theme, hint, &app.status, None);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ✦ ",
                Style::default()
                    .fg(app.theme.star)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "memo ",
                Style::default()
                    .fg(Color::Rgb(0x45, 0x47, 0x5a))
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        cols[1],
    );
}
