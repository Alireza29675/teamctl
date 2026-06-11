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
    let color = crate::term::use_color(err.is_terminal());
    let _ = writeln!(err, "{}", format_warning(&body, color));
}

/// Format the one-line warning, ANSI-styling the `warning:` prefix only
/// when `color` is true. Pulled out as a pure fn so the NO_COLOR / TTY
/// gate (via `crate::term::use_color`) is unit-testable without a real
/// terminal. (T-181)
fn format_warning(body: &str, color: bool) -> String {
    if color {
        format!("\x1b[33mwarning:\x1b[0m {body}")
    } else {
        format!("warning: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_warning;

    #[test]
    fn format_warning_styles_prefix_only_when_color_on() {
        let colored = format_warning("oops", true);
        assert!(
            colored.contains("\x1b[33mwarning:\x1b[0m oops"),
            "color path must style the prefix, got: {colored:?}"
        );

        let plain = format_warning("oops", false);
        assert_eq!(plain, "warning: oops");
        assert!(
            !plain.contains('\x1b'),
            "NO_COLOR / non-TTY output must carry no ANSI, got: {plain:?}"
        );
    }
}
