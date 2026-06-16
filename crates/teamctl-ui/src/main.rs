//! `teamctl-ui` binary entry. Sets up the terminal, runs the app loop, and
//! restores the terminal on every exit path — including panics.

use std::io::{self, stdout};
use std::panic;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<()> {
    // `--init-picker` is the interactive `teamctl init` template picker,
    // launched by `teamctl init`. It owns its own (stderr) terminal
    // lifecycle and prints the chosen key to stdout, so dispatch it before
    // the dashboard path.
    if std::env::args().nth(1).as_deref() == Some("--init-picker") {
        return run_init_picker();
    }
    // Handle `--version` / `--help` before any terminal setup so the binary
    // is callable from non-TTY contexts (CI smoke tests, scripts) without
    // tripping `enable_raw_mode()`.
    if handle_info_flags() {
        return Ok(());
    }
    install_panic_hook();
    enter_terminal()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = teamctl_ui::app::run(&mut terminal);
    leave_terminal()?;
    terminal.show_cursor()?;
    result
}

/// `teamctl-ui --init-picker [examples_dir]` — the interactive `teamctl
/// init` template picker. Renders to stderr and prints the chosen key to
/// stdout (so the parent `teamctl init` can capture it): a template/example
/// key, or `guided` for the co-design branch. Exits 130 on cancel.
fn run_init_picker() -> Result<()> {
    use teamctl_ui::init_picker::{run_standalone, Outcome};
    let examples_dir = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "examples".to_string());
    match run_standalone(std::path::Path::new(&examples_dir))? {
        Outcome::Selected(key) => {
            println!("{key}");
            Ok(())
        }
        Outcome::CoDesign => {
            println!("guided");
            Ok(())
        }
        Outcome::Cancelled => std::process::exit(130),
    }
}

fn handle_info_flags() -> bool {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("teamctl-ui {}", env!("CARGO_PKG_VERSION"));
            true
        }
        Some("--help" | "-h") => {
            println!("teamctl-ui {}", env!("CARGO_PKG_VERSION"));
            println!();
            println!(
                "Interactive TUI for teamctl — Triptych view, approvals modal, send-mail compose."
            );
            println!();
            println!("Usage: teamctl-ui [OPTIONS]");
            println!();
            println!("Options:");
            println!("  -h, --help     Print help");
            println!("  -V, --version  Print version");
            println!();
            println!("Run with no arguments to launch the TUI.");
            true
        }
        _ => false,
    }
}

fn enter_terminal() -> Result<()> {
    enable_raw_mode()?;
    // EnableMouseCapture routes wheel events through the TUI's own
    // event loop (T-158). Released in `leave_terminal` so the parent
    // shell regains normal mouse behaviour on every exit path —
    // including the panic hook below.
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    Ok(())
}

fn leave_terminal() -> Result<()> {
    let mut out = io::stdout();
    execute!(out, DisableMouseCapture, LeaveAlternateScreen)?;
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
