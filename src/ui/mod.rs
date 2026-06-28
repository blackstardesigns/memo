use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::theme::ResolvedTheme;

mod drawer;
mod editor;
mod list;
mod modal;

/// Draw the whole UI for the current frame.
pub fn draw(f: &mut Frame, app: &mut App) {
    // When open, the drawer takes a fixed-width column on the left and the active
    // screen renders in what remains.
    let main = if app.drawer_open {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(drawer::DRAWER_W), Constraint::Min(0)])
            .split(f.area());
        drawer::draw(f, app, cols[0]);
        cols[1]
    } else {
        f.area()
    };

    match app.screen {
        Screen::List => list::draw(f, app, main),
        Screen::Editor => editor::draw(f, app, main),
    }
    modal::draw(f, app);
}

/// Render the single-row bottom bar: an optional shortcuts hint (or, taking
/// priority, a transient status message) on the left, and an optional
/// right-aligned indicator (e.g. refinement status) on the right.
pub fn draw_bottom_bar(
    f: &mut Frame,
    area: Rect,
    theme: &ResolvedTheme,
    hint: Option<&str>,
    status_msg: &str,
    right: Option<(&str, Color)>,
) {
    let right_w = right
        .map(|(t, _)| t.chars().count() as u16 + 2)
        .unwrap_or(0);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_w)])
        .split(area);

    if !status_msg.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            format!(" {status_msg} "),
            Style::default().fg(theme.status),
        )));
        f.render_widget(p, cols[0]);
    } else if let Some(h) = hint {
        let p = Paragraph::new(Line::from(Span::styled(
            h,
            Style::default().fg(theme.footer_fg).bg(theme.footer_bg),
        )));
        f.render_widget(p, cols[0]);
    }

    if let Some((text, color)) = right {
        let p = Paragraph::new(Line::from(Span::styled(
            format!("{text} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Right);
        f.render_widget(p, cols[1]);
    }
}

/// Centered rectangle occupying `px`% width and `py`% height of `area`.
pub fn centered_rect(px: u16, py: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - py) / 2),
            Constraint::Percentage(py),
            Constraint::Percentage((100 - py) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - px) / 2),
            Constraint::Percentage(px),
            Constraint::Percentage((100 - px) / 2),
        ])
        .split(vertical[1])[1]
}

/// Truncate to `max` characters, appending an ellipsis when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let kept: String = chars.into_iter().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}
