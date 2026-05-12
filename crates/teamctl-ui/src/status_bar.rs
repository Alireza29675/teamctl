//! Bottom status bar — operator-orientation strip beneath the existing
//! keybindings statusline. Two slots:
//!
//! - **Left:** the team root (`app.team.root`) — what `.team/` directory
//!   the TUI was launched against. Smart-truncated to fit (`~/`-prefixed
//!   when under HOME, middle-ellipsis when over the slot's budget).
//! - **Right:** live system CPU% + RAM%, refreshed on the existing
//!   1-second App refresh tick (see `app::REFRESH_INTERVAL`). No
//!   background thread — sysinfo's per-tick refresh is sub-millisecond.
//!
//! - **Center:** the focused agent's claude rate-limit window when
//!   active, formatted as `limit 5m 12s` (T-212). Gated behind the
//!   `TEAMCTL_UI_RATE_LIMIT_INDICATOR=1` preview env var — opt-in
//!   while the indicator's shape stabilizes against the future
//!   usage-% data path. Hides when the focused agent has no active
//!   window; swaps with focus.
//!
//! Truncation priority on narrow terminals: **path > per-agent
//! center > CPU/RAM** (T-209 done-when, extended by T-212). Operators
//! who can't see WHERE the team is running lose more than operators
//! who can't see the per-agent indicator, who in turn lose more than
//! operators who can't see live CPU%. Matches the statusline
//! module's right-anchor-elides-first pattern from `statusline.rs`.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

use crate::app::App;
use crate::data::format_rate_limit_window;

/// Render the bottom status bar into the supplied `area`. Mirrors the
/// `statusline::draw` entry-point shape so the call site is uniform.
pub fn draw(f: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    StatusBar { app }.render(area, f.buffer_mut());
}

pub struct StatusBar<'a> {
    pub app: &'a App,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let muted = self.app.capabilities.muted();
        // System metrics are rendered as a single short string —
        // measured once and right-aligned so they hug the trailing
        // edge of the bar regardless of the path's length.
        let metrics = system_metrics_string(&self.app.sysinfo);

        // Truncation priority: path wins. Compute the path slot first
        // with the FULL width, then reserve metrics-width on the right
        // only if there's enough headroom after the path is rendered.
        let area_w = area.width as usize;
        let metrics_w = metrics.chars().count();
        let path_str = display_path(&self.app.team.root);

        // Reserve at least one space of gutter between path and
        // metrics if metrics will render. If the terminal is too
        // narrow for both, metrics elide entirely.
        let gutter = 1usize;
        let metrics_will_render = area_w >= path_min_width() + metrics_w + gutter;

        let path_budget = if metrics_will_render {
            area_w.saturating_sub(metrics_w + gutter)
        } else {
            area_w
        };
        let path_rendered = truncate_path_middle(&path_str, path_budget);

        // Render path at column 0.
        buf.set_string(area.x, area.y, &path_rendered, Style::default().fg(muted));

        // T-212: per-agent rate-limit indicator in the center slot.
        // Gated behind the `rate_limit_indicator_enabled` preview
        // flag (env: `TEAMCTL_UI_RATE_LIMIT_INDICATOR=1`) so the
        // operator-facing shape can stabilize alongside the
        // eventual usage-% data path before the indicator opts in
        // for everyone. When the flag is off, the slot stays blank
        // regardless of agent state — path + metrics layout is
        // unchanged from T-209 baseline.
        //
        // When enabled, renders ONLY when the focused agent has an
        // active rate-limit window (`format_rate_limit_window`
        // returns `Some` — past or unset windows yield `None`).
        // Truncation priority per the contract with otis on T-209:
        // path > per-agent > metrics. We honor it by measure-and-fit
        // — the slot renders only if there's room between the path's
        // rendered right edge (+gutter) and the metrics' x position
        // (-gutter). When the terminal is too narrow, the indicator
        // simply doesn't render; path and metrics keep their slots.
        let center_text: Option<String> = if self.app.rate_limit_indicator_enabled {
            self.app
                .selected_agent
                .and_then(|i| self.app.team.agents.get(i))
                .and_then(|a| {
                    let now_unix = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    format_rate_limit_window(a.rate_limit_resets_at, now_unix)
                })
                .map(|w| format!("limit {w}"))
        } else {
            None
        };

        let metrics_x_or_end = if metrics_will_render {
            area_w.saturating_sub(metrics_w)
        } else {
            area_w
        };
        if let Some(text) = center_text {
            let text_w = text.chars().count();
            let path_actual_w = path_rendered.chars().count();
            let center_left_bound = path_actual_w + gutter;
            let center_right_bound = metrics_x_or_end.saturating_sub(gutter);
            if center_right_bound > center_left_bound
                && text_w <= center_right_bound - center_left_bound
            {
                let avail = center_right_bound - center_left_bound;
                let pad = (avail - text_w) / 2;
                let center_x = area.x + (center_left_bound + pad) as u16;
                buf.set_string(center_x, area.y, &text, Style::default().fg(muted));
            }
        }

        if metrics_will_render {
            let metrics_x = area.x + (area_w as u16 - metrics_w as u16);
            buf.set_string(
                metrics_x,
                area.y,
                &metrics,
                Style::default().fg(muted).add_modifier(Modifier::DIM),
            );
        }
    }
}

/// Minimum number of columns we ever spend on the path before forcing
/// metrics to elide. Below this, the path itself starts truncating
/// (head/tail ellipsis) but it stays visible — losing path entirely
/// would be worse than losing live metrics.
fn path_min_width() -> usize {
    // Leaves room for at least `~/.../<basename>` shape (~12 chars).
    12
}

/// Render `path` as the operator-friendly string the status bar shows
/// when there's room: HOME-relative when applicable, full otherwise.
/// Truncation to fit a budget happens separately via
/// [`truncate_path_middle`].
fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            // Bare `~` with no trailing slash if the team root IS home.
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Smart-truncate a path string to at most `max_width` chars. Strategy:
///
/// - If the string fits, return it as-is.
/// - If it doesn't, keep the leading prefix (so HOME-relative `~/...`
///   stays recognizable) and the trailing basename (so the operator
///   sees what they launched against), with a middle `…` separator.
/// - If even basename-only doesn't fit, hard-clip from the left and
///   prefix with `…`.
///
/// Counts USVs (`chars()`), not bytes — paths can contain multi-byte
/// chars. Ratatui's `set_string` measures display width, so this gives
/// a tight upper bound but is an OK approximation for ASCII paths.
fn truncate_path_middle(path: &str, max_width: usize) -> String {
    let total = path.chars().count();
    if total <= max_width {
        return path.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    // Head/tail split — keep the basename intact when possible.
    // Reserve 1 char for the middle ellipsis.
    let budget = max_width - 1;
    // Find the basename (last `/`-segment) length.
    let basename_len = path
        .rsplit('/')
        .next()
        .map(|s| s.chars().count())
        .unwrap_or(0);
    if basename_len + 4 < budget {
        // We have room for `<head>…<basename>`; spend the budget as
        // head = budget - basename_len, tail = basename_len.
        let head_len = budget - basename_len;
        let head: String = path.chars().take(head_len).collect();
        let tail: String = path
            .chars()
            .rev()
            .take(basename_len)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return format!("{head}…{tail}");
    }
    // Basename alone overflows — hard-clip with leading ellipsis.
    let tail: String = path
        .chars()
        .rev()
        .take(budget)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

/// Compose the metrics string from a refreshed [`sysinfo::System`].
/// Shape: `CPU 12% · RAM 4.2/16 GB`. Compact enough to fit alongside
/// the path on common terminal widths (≥ 80 cols). The dot separator
/// matches the statusline module's `·`-joined hint convention.
fn system_metrics_string(sys: &sysinfo::System) -> String {
    let cpu = global_cpu_percent(sys);
    let (used_gb, total_gb) = ram_used_total_gb(sys);
    format!("CPU {cpu}% · RAM {used_gb:.1}/{total_gb:.0} GB")
}

fn global_cpu_percent(sys: &sysinfo::System) -> u8 {
    // `global_cpu_usage` is a 0..100 f32 representing the aggregate
    // across all cores. Round to the nearest u8 — the status bar
    // doesn't have room for decimal precision and the operator
    // doesn't need it.
    sys.global_cpu_usage().round().clamp(0.0, 100.0) as u8
}

fn ram_used_total_gb(sys: &sysinfo::System) -> (f32, f32) {
    // sysinfo reports memory in BYTES on 0.32+. Convert to gigabytes
    // (decimal — `10^9`, matching what most operators see in their
    // system monitors) for display.
    const GB: f32 = 1_000_000_000.0;
    let used = sys.used_memory() as f32 / GB;
    let total = sys.total_memory() as f32 / GB;
    (used, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_path_middle_returns_path_unchanged_when_it_fits() {
        assert_eq!(
            truncate_path_middle("/home/user/proj", 32),
            "/home/user/proj"
        );
    }

    #[test]
    fn truncate_path_middle_preserves_basename_with_head_ellipsis() {
        // Budget tight enough to force truncation but loose enough
        // for head+tail+ellipsis. Basename `teamctl` stays visible.
        let truncated = truncate_path_middle("/home/alireza/dev/projects/teamctl", 20);
        assert!(
            truncated.ends_with("teamctl"),
            "basename lost: {truncated:?}"
        );
        assert!(truncated.contains('…'), "no ellipsis: {truncated:?}");
        assert!(truncated.chars().count() <= 20);
    }

    #[test]
    fn truncate_path_middle_hard_clips_when_basename_overflows() {
        // Budget smaller than the basename — fall back to right-anchor
        // hard-clip with leading ellipsis.
        let truncated =
            truncate_path_middle("/home/user/extremely-long-project-directory-name", 12);
        assert!(
            truncated.starts_with('…'),
            "no leading ellipsis: {truncated:?}"
        );
        assert!(truncated.chars().count() <= 12);
    }

    #[test]
    fn truncate_path_middle_degenerate_widths() {
        assert_eq!(truncate_path_middle("/long/path", 0), "…");
        assert_eq!(truncate_path_middle("/long/path", 1), "…");
    }

    #[test]
    fn truncate_path_middle_counts_chars_not_bytes() {
        // Multi-byte char in the path. `é` is 2 bytes in UTF-8 but
        // one display column for our purposes.
        let truncated = truncate_path_middle("/home/usér/projet", 13);
        assert!(truncated.chars().count() <= 13);
    }

    #[test]
    fn display_path_collapses_home_prefix() {
        // We can only test this when HOME is resolvable; if not,
        // skip (CI runners always set HOME).
        if let Some(home) = dirs::home_dir() {
            let under_home = home.join("dev/projects/teamctl/.team");
            let rendered = display_path(&under_home);
            assert!(
                rendered.starts_with("~/"),
                "expected ~-prefix: {rendered:?}"
            );
            assert!(rendered.ends_with("teamctl/.team"));
        }
    }

    #[test]
    fn display_path_returns_full_path_outside_home() {
        let outside = Path::new("/tmp/teamctl-fixture/.team");
        let rendered = display_path(outside);
        assert_eq!(rendered, "/tmp/teamctl-fixture/.team");
    }

    #[test]
    fn display_path_handles_path_equal_to_home() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(display_path(&home), "~");
        }
    }

    #[test]
    fn metrics_string_is_compact_and_well_formed() {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        let s = system_metrics_string(&sys);
        assert!(s.starts_with("CPU "), "metrics shape changed: {s:?}");
        assert!(s.contains(" · RAM "), "separator missing: {s:?}");
        assert!(s.ends_with(" GB"), "trailing unit missing: {s:?}");
        // ≤ 30 chars at typical sizes — leaves room for the path.
        assert!(s.chars().count() < 30, "metrics too wide: {s:?}");
    }

    #[test]
    fn path_min_width_is_reasonable() {
        // Sanity-pin: the minimum reservation should be wide enough
        // for `~/…/basename` shape (~10-12 chars).
        assert!(path_min_width() >= 10);
        assert!(path_min_width() <= 16);
    }
}
