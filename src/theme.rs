use ratatui::style::Color;

use crate::config::Theme;

/// Theme with config color strings resolved to concrete [`Color`]s.
#[derive(Clone)]
pub struct ResolvedTheme {
    pub accent: Color,
    pub border: Color,
    pub refined: Color,
    pub star: Color,
    pub title_fg: Color,
    pub title_bg: Color,
    pub footer_fg: Color,
    pub footer_bg: Color,
    pub status: Color,
    pub padding: u16,
    pub divider: Color,
    pub rounded_tiles: bool,
    pub meta: Color,
}

impl ResolvedTheme {
    pub fn from_config(t: &Theme) -> Self {
        ResolvedTheme {
            accent: parse_color(&t.accent, Color::Yellow),
            border: parse_color(&t.border, Color::DarkGray),
            refined: parse_color(&t.refined, Color::Magenta),
            star: parse_color(&t.star, Color::Magenta),
            title_fg: parse_color(&t.title_fg, Color::White),
            title_bg: parse_color(&t.title_bg, Color::Blue),
            footer_fg: parse_color(&t.footer_fg, Color::Black),
            footer_bg: parse_color(&t.footer_bg, Color::Gray),
            status: parse_color(&t.status, Color::Green),
            padding: t.padding,
            divider: parse_color(&t.divider, Color::DarkGray),
            rounded_tiles: t.rounded_tiles,
            meta: parse_color(&t.meta, Color::DarkGray),
        }
    }
}

/// Parse a color from a name, `#rrggbb` hex, or a 0–255 palette index.
/// Falls back to `fallback` on anything unrecognized.
pub fn parse_color(s: &str, fallback: Color) -> Color {
    let s = s.trim();
    if s.is_empty() {
        return fallback;
    }
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            if let Ok(n) = u32::from_str_radix(hex, 16) {
                return Color::Rgb(
                    ((n >> 16) & 0xff) as u8,
                    ((n >> 8) & 0xff) as u8,
                    (n & 0xff) as u8,
                );
            }
        }
        return fallback;
    }
    if let Ok(idx) = s.parse::<u8>() {
        return Color::Indexed(idx);
    }
    match s.to_ascii_lowercase().as_str() {
        // Use the terminal's default color (transparent background / default text).
        "none" | "transparent" | "reset" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_hex_index_and_fallback() {
        assert_eq!(parse_color("cyan", Color::Black), Color::Cyan);
        assert_eq!(parse_color("DarkGray", Color::Black), Color::DarkGray);
        assert_eq!(
            parse_color("#ff8800", Color::Black),
            Color::Rgb(255, 136, 0)
        );
        assert_eq!(parse_color("5", Color::Black), Color::Indexed(5));
        assert_eq!(parse_color("none", Color::Red), Color::Reset);
        assert_eq!(parse_color("transparent", Color::Red), Color::Reset);
        assert_eq!(parse_color("bogus", Color::Red), Color::Red);
        assert_eq!(parse_color("", Color::Red), Color::Red);
    }
}
