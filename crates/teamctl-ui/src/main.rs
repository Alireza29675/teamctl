//! `teamctl-ui` binary entry. Sets up the terminal, runs the app loop, and
//! restores the terminal on every exit path — including panics.

use std::io::{self, stdout};
use std::panic;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<()> {
    install_panic_hook();
    enter_terminal()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = teamctl_ui::app::run(&mut terminal);
    leave_terminal()?;
    terminal.show_cursor()?;
    result
}

fn enter_terminal() -> Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    Ok(())
}

fn leave_terminal() -> Result<()> {
    let mut out = io::stdout();
    execute!(out, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

/// Restore the terminal before the default panic handler dumps the
/// backtrace, otherwise the operator's shell ends up in raw mode with
/// the alternate screen still active.
fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = leave_terminal();
        original(info);
    }));
}
