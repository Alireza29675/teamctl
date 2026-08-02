//! `teamctl-ui` binary entry. Sets up the terminal, runs the app loop, and
//! restores the terminal on every exit path — including panics.

use std::io::{self, stdout};
use std::panic;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<()> {
    // `--init-picker` is the interactive `teamctl init` template picker,
    // launched by `teamctl init`. It owns its own (stderr) terminal
    // lifecycle and prints a PickerResponse JSON line to stdout, so dispatch
    // it before the dashboard path.
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

/// `teamctl-ui --init-picker --catalog <path>` — the interactive `teamctl
/// init` template picker. The catalog is decoded and validated before any
/// terminal setup. The picker renders to stderr and prints one
/// `PickerResponse` JSON line to stdout; Esc / q / Ctrl-C exits 130.
fn run_init_picker() -> Result<()> {
    use teamctl_ui::init_picker::{run_standalone, Outcome};
    let args: Vec<String> = std::env::args().skip(2).collect();
    let catalog_path = parse_catalog_path(&args)?;
    // Keep this above `run_standalone`: malformed/version-skewed handoffs
    // must fail cleanly without touching raw mode or the alternate screen.
    let catalog = read_catalog(&catalog_path)?;

    match run_standalone(catalog.entries)? {
        Outcome::Selected(response) => {
            println!("{}", response_json(&response)?);
            Ok(())
        }
        Outcome::Cancelled => std::process::exit(130),
    }
}

fn parse_catalog_path(args: &[String]) -> Result<PathBuf> {
    match args {
        [flag, path] if flag == "--catalog" && !path.is_empty() => Ok(PathBuf::from(path)),
        _ => bail!("usage: teamctl-ui --init-picker --catalog <path>"),
    }
}

fn read_catalog(path: &Path) -> Result<team_core::preview::PickerCatalog> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read picker catalog at {}", path.display()))?;
    let catalog: team_core::preview::PickerCatalog = serde_json::from_str(&body)
        .with_context(|| format!("decode picker catalog at {}", path.display()))?;
    catalog
        .validate()
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("validate picker catalog at {}", path.display()))?;
    Ok(catalog)
}

fn response_json(response: &team_core::preview::PickerResponse) -> Result<String> {
    serde_json::to_string(response).context("encode picker response")
}

fn handle_info_flags() -> bool {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("teamctl-ui {}", env!("CARGO_PKG_VERSION"));
            if std::env::args().nth(2).as_deref()
                == Some(team_core::preview::PICKER_PROTOCOL_VERSION_ARG)
            {
                println!("{}", team_core::preview::PICKER_PROTOCOL_VERSION);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use team_core::preview::{
        PickerCatalog, PickerCatalogEntry, PickerResponse, PreviewCounts, ShapeKind, ShapeRow,
        PICKER_PROTOCOL_VERSION,
    };

    fn catalog() -> PickerCatalog {
        PickerCatalog {
            version: PICKER_PROTOCOL_VERSION,
            entries: vec![PickerCatalogEntry {
                key: "essentials".into(),
                name: "Essentials".into(),
                description: "A useful starting team".into(),
                rows: vec![ShapeRow {
                    depth: 0,
                    kind: ShapeKind::Root,
                    label: "You".into(),
                    descriptor: String::new(),
                    is_last: true,
                }],
                counts: PreviewCounts::default(),
            }],
        }
    }

    #[test]
    fn init_picker_args_require_exact_catalog_path() {
        assert_eq!(
            parse_catalog_path(&["--catalog".into(), "/tmp/catalog.json".into()]).unwrap(),
            PathBuf::from("/tmp/catalog.json")
        );
        for invalid in [
            vec![],
            vec!["--catalog".into()],
            vec!["examples".into()],
            vec!["--catalog".into(), "".into()],
            vec!["--catalog".into(), "catalog.json".into(), "extra".into()],
        ] {
            assert!(
                parse_catalog_path(&invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn picker_catalog_is_read_and_validated_before_terminal_setup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.json");
        std::fs::write(&path, serde_json::to_vec(&catalog()).unwrap()).unwrap();

        assert_eq!(read_catalog(&path).unwrap(), catalog());
    }

    #[test]
    fn malformed_picker_catalog_is_rejected_before_terminal_setup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.json");
        std::fs::write(&path, "{not json").unwrap();

        let error = read_catalog(&path).unwrap_err().to_string();
        assert!(error.contains("decode picker catalog"), "{error}");
    }

    #[test]
    fn unsupported_picker_catalog_is_rejected_before_terminal_setup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("catalog.json");
        let mut invalid = catalog();
        invalid.version += 1;
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();

        let error = read_catalog(&path).unwrap_err().to_string();
        assert!(error.contains("validate picker catalog"), "{error}");
    }

    #[test]
    fn picker_responses_encode_as_one_json_value() {
        let cases = [
            (
                PickerResponse::Create {
                    key: "essentials".into(),
                },
                r#"{"action":"create","key":"essentials"}"#,
            ),
            (
                PickerResponse::Customize {
                    key: "essentials".into(),
                },
                r#"{"action":"customize","key":"essentials"}"#,
            ),
            (PickerResponse::CoDesign, r#"{"action":"co_design"}"#),
        ];

        for (response, expected) in cases {
            let json = response_json(&response).unwrap();
            assert_eq!(json, expected);
            assert!(!json.contains('\n'));
        }
    }
}
