//! Source-aware warning when the resolved compose root did not come from
//! the operator's CWD walk-up (T-010). Goal: an operator running
//! introspection commands never silently inspects a *different* team than
//! the one their CWD implies.
//!
//! The warning fires for `validate`, `ps`, `mail`, `inspect` whenever the
//! root was picked up from `TEAMCTL_ROOT`. It is suppressed when the
//! operator passed `--root` explicitly (deliberate intent) or when
//! `TEAMCTL_QUIET=1` is set (script escape hatch). Registered-context
//! resolution was retired in T-008.

use std::io::{IsTerminal, Write};
use std::path::Path;

#[derive(Debug, Clone)]
pub enum RootSource {
    /// `--root` / `-C` passed explicitly on the command line.
    CliFlag,
    /// `TEAMCTL_ROOT` environment variable.
    Env,
    /// Walked up from CWD looking for `.team/team-compose.yaml`.
    WalkUp,
}

/// Print a one-line warning to stderr if `source` is something other than
/// CWD walk-up or an explicit `--root`. No-op when `TEAMCTL_QUIET=1`.
pub fn maybe_warn_root_source(source: &RootSource, root: &Path) {
    if std::env::var_os("TEAMCTL_QUIET")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        return;
    }
    let body = match source {
        RootSource::CliFlag | RootSource::WalkUp => return,
        RootSource::Env => format!(
            "using $TEAMCTL_ROOT={} (CWD walk-up would resolve elsewhere or fail)",
            root.display()
        ),
    };
    let mut err = std::io::stderr().lock();
    if err.is_terminal() {
        let _ = writeln!(err, "\x1b[33mwarning:\x1b[0m {body}");
    } else {
        let _ = writeln!(err, "warning: {body}");
    }
}
