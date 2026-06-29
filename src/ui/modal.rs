use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{filter_symbols, App, Modal};
use crate::ui::centered_rect;

pub fn draw(f: &mut Frame, app: &App) {
    let accent = app.theme.accent;
    match &app.modal {
        Modal::None => {}
        Modal::Help => draw_help(f, app),
        Modal::ConfirmDelete => draw_box(
            f,
            " Confirm delete ",
            "Delete this note?\nThis cannot be undone.\n\n[y] yes     [n] no",
            Color::Red,
        ),
        Modal::ConfirmDeleteFolder(id) => {
            let (title, count) = app
                .folders
                .iter()
                .find(|fd| &fd.id == id)
                .map(|fd| (fd.title.clone(), app.folder_notes(id).len()))
                .unwrap_or_else(|| ("this folder".to_string(), 0));
            let notes = if count == 1 { "note" } else { "notes" };
            let body = format!(
                "Delete folder “{title}”?\nIts {count} {notes} will move to the top level.\n\n[y] yes     [n] no"
            );
            draw_box(f, " Delete folder ", &body, Color::Red);
        }
        Modal::Error(msg) => draw_box(f, " Error ", msg, Color::Red),
        Modal::Info(msg) => draw_box(f, " Done ", msg, app.theme.status),
        Modal::TitleEdit { buf, cursor } => draw_title_edit(f, app, buf, *cursor),
        Modal::Export(buf) => draw_input(
            f,
            " Export path — Enter to save, Esc to cancel ",
            buf,
            accent,
        ),
        Modal::NewFolder(buf) => draw_input(
            f,
            " New folder — Enter to create, Esc to cancel ",
            buf,
            accent,
        ),
        Modal::MoveNote { sel, .. } => draw_move(f, app, *sel, accent),
        Modal::CustomPrompt(buf) => draw_custom_prompt(f, app, buf),
        Modal::SymbolPicker { query, sel } => draw_symbol_picker(f, app, query, *sel),
        Modal::ConfirmQuit => draw_box(
            f,
            " Quit note? ",
            "[y] yes     [n] no",
            Color::Red,
        ),
    }
}

/// A selectable list of move destinations: "top level" plus every folder.
fn draw_move(f: &mut Frame, app: &App, sel: usize, accent: Color) {
    let area = centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);

    let mut rows: Vec<Line> = Vec::with_capacity(app.folders.len() + 1);
    let mut push_row = |idx: usize, label: String| {
        let selected = idx == sel;
        let marker = if selected { "› " } else { "  " };
        let style = if selected {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        rows.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    };
    push_row(0, "None (top level)".to_string());
    for (i, folder) in app.folders.iter().enumerate() {
        push_row(i + 1, folder.title.clone());
    }

    let para = Paragraph::new(Text::from(rows))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Move to folder — ↑↓ choose, Enter confirm, Esc cancel ")
                .border_style(Style::default().fg(accent)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_custom_prompt(f: &mut Frame, app: &App, buf: &str) {
    let area = centered_rect(62, 38, f.area());
    f.render_widget(Clear, area);

    let theme = &app.theme;
    let border_type = if theme.rounded_tiles {
        BorderType::Rounded
    } else {
        BorderType::Plain
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // ✦ header
            Constraint::Length(1), // divider
            Constraint::Min(0),    // prompt text
            Constraint::Length(1), // footer hint
        ])
        .split(inner);

    // Header
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ✦ ",
                Style::default().fg(Color::Rgb(127, 0, 255)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "custom refine prompt",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        chunks[0],
    );

    // Thin divider
    let divider = "─".repeat(inner.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(divider, Style::default().fg(theme.divider))),
        chunks[1],
    );

    // Prompt body with cursor
    let mut text = buf.to_string();
    text.push('▏');
    f.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: false })
            .block(Block::default().padding(Padding::new(1, 1, 1, 0))),
        chunks[2],
    );

    // Footer
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Enter to refine · Esc to cancel",
            Style::default().fg(theme.border),
        ))),
        chunks[3],
    );
}

fn draw_title_edit(f: &mut Frame, app: &App, buf: &str, cursor: usize) {
    let area = centered_rect(62, 30, f.area());
    f.render_widget(Clear, area);

    let theme = &app.theme;
    let border_type = if theme.rounded_tiles {
        BorderType::Rounded
    } else {
        BorderType::Plain
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // ✦ header
            Constraint::Length(1), // divider
            Constraint::Min(0),    // input
            Constraint::Length(1), // footer hint
        ])
        .split(inner);

    // Header
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ✦ ",
                Style::default().fg(Color::Rgb(127, 0, 255)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "edit title",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        chunks[0],
    );

    // Divider
    let divider = "─".repeat(inner.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(divider, Style::default().fg(theme.divider))),
        chunks[1],
    );

    // Input — render plain text and place the terminal block cursor.
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            buf.to_string(),
            Style::default().fg(Color::Gray),
        )))
        .block(Block::default().padding(Padding::new(1, 1, 1, 0))),
        chunks[2],
    );
    let text_x = chunks[2].x + 1; // left padding = 1
    let text_y = chunks[2].y + 1; // top padding = 1
    let max_x = chunks[2].x + chunks[2].width.saturating_sub(2);
    f.set_cursor_position(((text_x + cursor as u16).min(max_x), text_y));

    // Footer
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Enter save · Esc cancel",
            Style::default().fg(theme.border),
        ))),
        chunks[3],
    );
}

fn draw_box(f: &mut Frame, title: &str, body: &str, color: Color) {
    let area = centered_rect(60, 30, f.area());
    f.render_widget(Clear, area);
    let para = Paragraph::new(body)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(color)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_input(f: &mut Frame, title: &str, buf: &str, accent: Color) {
    let area = centered_rect(70, 20, f.area());
    f.render_widget(Clear, area);
    let mut text = buf.to_string();
    text.push('▏');
    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(accent)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_symbol_picker(f: &mut Frame, app: &App, query: &str, sel: usize) {
    let area = centered_rect(54, 72, f.area());
    f.render_widget(Clear, area);

    let theme = &app.theme;
    let border_type = if theme.rounded_tiles {
        BorderType::Rounded
    } else {
        BorderType::Plain
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // divider
            Constraint::Length(1), // search input
            Constraint::Length(1), // divider
            Constraint::Min(0),    // symbol list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // Header
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ∑ ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "symbol picker",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        chunks[0],
    );

    let divider = "─".repeat(inner.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(divider.clone(), Style::default().fg(theme.divider))),
        chunks[1],
    );

    // Search input
    let input_display = format!(" > {}▏", query);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            input_display,
            Style::default().fg(Color::Gray),
        ))),
        chunks[2],
    );

    f.render_widget(
        Paragraph::new(Span::styled(divider, Style::default().fg(theme.divider))),
        chunks[3],
    );

    // Symbol list
    let filtered = filter_symbols(query);
    let list_height = chunks[4].height as usize;
    let scroll = if sel + 1 > list_height {
        sel + 1 - list_height
    } else {
        0
    };

    let rows: Vec<Line> = filtered
        .iter()
        .enumerate()
        .skip(scroll)
        .take(list_height)
        .map(|(i, &(ch, name))| {
            let selected = i == sel;
            let marker = if selected { "▸ " } else { "  " };
            let style = if selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let ch_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            Line::from(vec![
                Span::styled(marker.to_string(), style),
                Span::styled(format!("{ch}  "), ch_style),
                Span::styled(name.to_string(), style),
            ])
        })
        .collect();

    let empty_msg = if filtered.is_empty() {
        vec![Line::from(Span::styled(
            "  no matches",
            Style::default().fg(theme.border),
        ))]
    } else {
        vec![]
    };

    f.render_widget(
        Paragraph::new(if rows.is_empty() { empty_msg } else { rows }),
        chunks[4],
    );

    // Footer
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Enter insert · Esc cancel · ↑↓ navigate",
            Style::default().fg(theme.border),
        ))),
        chunks[5],
    );
}

fn draw_help(f: &mut Frame, app: &App) {
    let area = centered_rect(66, 84, f.area());
    f.render_widget(Clear, area);

    let theme = &app.theme;
    let border_type = if theme.rounded_tiles {
        BorderType::Rounded
    } else {
        BorderType::Plain
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    // Header
    let version_str = format!("v{} ", env!("CARGO_PKG_VERSION"));
    let left_visual_len = 7u16; // " ✦ help"
    let padding = inner
        .width
        .saturating_sub(left_visual_len + version_str.len() as u16);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ✦ ",
                Style::default().fg(Color::Rgb(127, 0, 255)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "help",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(padding as usize)),
            Span::styled(version_str, Style::default().fg(theme.border)),
        ])),
        chunks[0],
    );

    // Divider
    let divider = "─".repeat(inner.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(divider, Style::default().fg(theme.divider))),
        chunks[1],
    );

    // Content
    let head = |s: &str| {
        Line::from(Span::styled(
            format!("  {s}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let bind = |k: &str, desc: &str| {
        Line::from(vec![
            Span::raw("    "),
            Span::styled(
                format!("{:<16}", k),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc.to_string(), Style::default().fg(theme.border)),
        ])
    };

    let lines = Text::from(vec![
        Line::raw(""),
        head("List view"),
        bind("n", "new note"),
        bind("Ctrl+F", "new folder"),
        bind("Enter / o", "open note or enter folder"),
        bind("m", "move note to a folder"),
        bind("/", "search"),
        bind("↑ ↓ ← →", "move selection"),
        bind("x", "delete note or folder"),
        bind("Ctrl+E", "export to .md"),
        bind("d", "toggle drawer"),
        bind("Ctrl+H", "help"),
        bind("q / Esc", "quit"),
        Line::raw(""),
        head("Editor"),
        bind("Ctrl+R", "refine with AI"),
        bind("Ctrl+P", "refine with custom prompt"),
        bind("Ctrl+M", "insert math symbol (∑ picker)"),
        bind("Tab", "toggle original / refined"),
        bind("Ctrl+T", "edit title"),
        bind("Ctrl+E", "export to .md"),
        bind("Ctrl+H", "help"),
        bind("Esc", "save & return to list"),
        Line::raw(""),
        head("Drawer"),
        bind("↑ ↓", "navigate"),
        bind("→ ←", "expand / collapse folder"),
        bind("Enter", "open note or toggle folder"),
        bind("Tab", "back to tiles"),
        bind("d / Esc", "close drawer"),
    ]);

    f.render_widget(Paragraph::new(lines), chunks[2]);

    // Footer
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " any key to close",
            Style::default().fg(theme.border),
        ))),
        chunks[3],
    );
}
