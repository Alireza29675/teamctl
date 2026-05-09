// Smoke coverage for the non-TTY entry points: `--version` and `--help`
// must exit 0 and print the version string before the binary tries to
// enable raw mode. The CI installer smoke job runs `teamctl-ui --version`
// to prove the binary is on PATH and runnable; if these arms regress
// (e.g. raw-mode setup creeps back above the early-return), CI catches it.

use std::process::Command;

fn bin() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_teamctl-ui").into()
}

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(
        out.status.success(),
        "--version exit code {:?} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let expected = format!("teamctl-ui {}", env!("CARGO_PKG_VERSION"));
    assert!(
        stdout.contains(&expected),
        "expected `{expected}` in --version output; got:\n{stdout}",
    );
}

#[test]
fn short_version_flag_prints_version_and_exits_zero() {
    let out = Command::new(bin()).arg("-V").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_flag_prints_usage_and_exits_zero() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(
        out.status.success(),
        "--help exit code {:?} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    // Pin the version line + the Usage section so future help-text drift
    // can't silently break the contract that `--help` is non-TTY-safe.
    assert!(
        stdout.contains(&format!("teamctl-ui {}", env!("CARGO_PKG_VERSION"))),
        "help missing version line: {stdout}",
    );
    assert!(
        stdout.contains("Usage: teamctl-ui"),
        "help missing Usage section: {stdout}",
    );
}
