//! Terminal capability detection — picks the colour fidelity teamctl-ui
//! renders at. Read once at startup; the rest of the UI passes the
//! `Capabilities` value down to widgets so colour choices stay
//! consistent across a single session even if the env mutates.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// `COLORTERM=truecolor|24bit` → 24-bit RGB available.
    TrueColor,
    /// `TERM=*-256color` → 256-colour palette.
    Palette256,
    /// Generic ANSI `TERM` (e.g. `xterm`, `screen`) → 16-colour palette.
    Ansi16,
    /// `TERM=dumb` or `NO_COLOR` set → render in plain glyphs.
    Monochrome,
}

#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub color: ColorMode,
}

impl Capabilities {
    /// Foreground accent for focus rings, statusline keybindings, and
    /// the splash logo. Falls back to plain `Reset` when colour is
    /// disabled so widgets don't have to branch on `ColorMode`.
    pub fn accent(self) -> Color {
        match self.color {
            ColorMode::TrueColor => Color::Rgb(0xfb, 0x73, 0x85),
            ColorMode::Palette256 => Color::Indexed(204),
            ColorMode::Ansi16 => Color::LightMagenta,
            ColorMode::Monochrome => Color::Reset,
        }
    }

    /// Dim text colour for empty-state placeholders and inactive
    /// statusline hints.
    pub fn muted(self) -> Color {
        match self.color {
            ColorMode::TrueColor => Color::Rgb(0x88, 0x88, 0x88),
            ColorMode::Palette256 => Color::Indexed(244),
            ColorMode::Ansi16 => Color::DarkGray,
            ColorMode::Monochrome => Color::Reset,
        }
    }
}

/// Detect terminal colour fidelity from environment variables. Honors
/// `NO_COLOR` (unconditional monochrome — see https://no-color.org)
/// and `TERM=dumb` first, then `COLORTERM`, then `TERM` substring.
pub fn detect_capabilities() -> Capabilities {
    Capabilities {
        color: detect_color_from_env(|k| std::env::var(k).ok()),
    }
}

fn detect_color_from_env<F: Fn(&str) -> Option<String>>(get: F) -> ColorMode {
    if get("NO_COLOR").is_some() {
        return ColorMode::Monochrome;
    }
    let term = get("TERM").unwrap_or_default();
    if term == "dumb" || term.is_empty() {
        return ColorMode::Monochrome;
    }
    if let Some(ct) = get("COLORTERM") {
        let lower = ct.to_ascii_lowercase();
        if lower == "truecolor" || lower == "24bit" {
            return ColorMode::TrueColor;
        }
    }
    if term.contains("256color") {
        return ColorMode::Palette256;
    }
    ColorMode::Ansi16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn no_color_wins_over_everything() {
        let mode = detect_color_from_env(env(&[
            ("NO_COLOR", "1"),
            ("COLORTERM", "truecolor"),
            ("TERM", "xterm-256color"),
        ]));
        assert_eq!(mode, ColorMode::Monochrome);
    }

    #[test]
    fn dumb_term_is_monochrome() {
        assert_eq!(
            detect_color_from_env(env(&[("TERM", "dumb")])),
            ColorMode::Monochrome
        );
    }

    #[test]
    fn truecolor_when_colorterm_set() {
        assert_eq!(
            detect_color_from_env(env(&[
                ("COLORTERM", "truecolor"),
                ("TERM", "xterm-256color"),
            ])),
            ColorMode::TrueColor
        );
    }

    #[test]
    fn palette256_when_term_says_so() {
        assert_eq!(
            detect_color_from_env(env(&[("TERM", "screen-256color")])),
            ColorMode::Palette256
        );
    }

    #[test]
    fn ansi16_for_plain_term() {
        assert_eq!(
            detect_color_from_env(env(&[("TERM", "xterm")])),
            ColorMode::Ansi16
        );
    }

    #[test]
    fn empty_term_is_monochrome() {
        assert_eq!(detect_color_from_env(env(&[])), ColorMode::Monochrome);
    }
}
