//! Golden-snapshot tests for the PR-UI-1 layout. Each test pins the
//! visible glyphs at a specific terminal size; insta diffs the
//! committed `*.snap` against the rendered buffer. Update with
//! `cargo insta review` when intentional layout changes land.
//!
//! Snapshots are intentionally rendered in monochrome (NO_COLOR=1
//! before `App::new`) so style sequences don't pollute the diff —
//! glyph layout is what we're pinning, not colour fidelity.

use ratatui::buffer::Buffer;
use teamctl_ui::app::{render_to_buffer, App, Stage};
use teamctl_ui::triptych::Pane;

fn buffer_to_string(buf: &Buffer) -> String {
    let area = buf.area();
    let mut out = String::with_capacity((area.width as usize + 1) * area.height as usize);
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(area.x + x, area.y + y)];
            out.push_str(cell.symbol());
        }
        out.push('\n');
    }
    out
}

fn fresh_app() -> App {
    // Force monochrome so snapshots don't capture ANSI colour state.
    std::env::set_var("NO_COLOR", "1");
    App::new()
}

#[test]
fn splash_layout_at_120x30() {
    let app = fresh_app();
    assert_eq!(app.stage, Stage::Splash);
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("splash_120x30", buffer_to_string(&buf));
}

#[test]
fn triptych_empty_state_at_120x30() {
    let mut app = fresh_app();
    app.dismiss_splash();
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("triptych_empty_120x30", buffer_to_string(&buf));
}

#[test]
fn triptych_focus_ring_follows_focused_pane() {
    let mut app = fresh_app();
    app.dismiss_splash();
    app.cycle_focus(); // Roster → Detail
    assert_eq!(app.focused_pane, Pane::Detail);
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("triptych_detail_focused_120x30", buffer_to_string(&buf));
}

#[test]
fn quit_confirm_overlay_at_120x30() {
    let mut app = fresh_app();
    app.dismiss_splash();
    app.enter_quit_confirm();
    let buf = render_to_buffer(&app, 120, 30);
    insta::assert_snapshot!("quit_confirm_120x30", buffer_to_string(&buf));
}

#[test]
fn statusline_renders_tutorial_hint_at_right() {
    // The `· t tutorial` hint is always visible (SPEC §4); pin it
    // here at a narrow width to catch regressions where it gets
    // pushed off-screen by a wider left-side hint.
    let mut app = fresh_app();
    app.dismiss_splash();
    let buf = render_to_buffer(&app, 80, 10);
    let s = buffer_to_string(&buf);
    let last_line = s.lines().last().expect("buffer not empty");
    assert!(
        last_line.contains("t tutorial"),
        "statusline missing tutorial hint at 80 cols: {last_line:?}"
    );
}

#[test]
fn render_at_minimum_terminal_does_not_panic() {
    // Small terminal — ratatui swallows over-large constraints, so as
    // long as the call doesn't panic we're good. (Smaller than ~16 wide
    // is degenerate; this pins the floor we care about.)
    let mut app = fresh_app();
    app.dismiss_splash();
    let _ = render_to_buffer(&app, 20, 8);
}
