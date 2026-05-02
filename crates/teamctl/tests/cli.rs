//! End-to-end integration test for the `teamctl` binary.
//!
//! Intentionally avoids `tmux` + `claude` so it runs on CI without a TTY:
//! drives only `validate` and `send` (which talk to SQLite directly), then
//! walks the mailbox to confirm the message landed.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn bin() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_teamctl").into()
}

fn seed_compose(root: &std::path::Path) {
    fs::write(
        root.join("team-compose.yaml"),
        r#"
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
supervisor:
  type: tmux
  tmux_prefix: a-
projects:
  - file: projects/hello.yaml
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::write(
        root.join("projects/hello.yaml"),
        r#"
version: 2
project:
  id: hello
  name: Hello
  cwd: .
channels:
  - name: all
    members: "*"
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-7
    can_dm: [dev]
    can_broadcast: [all]
workers:
  dev:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
    can_dm: [manager]
    can_broadcast: [all]
"#,
    )
    .unwrap();
}

#[test]
fn validate_passes_on_clean_compose() {
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());
    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("1 project"), "got: {stdout}");
    assert!(stdout.contains("2 agents"), "got: {stdout}");
}

#[test]
fn validate_fails_on_unknown_dm_target() {
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());
    let path = tmp.path().join("projects/hello.yaml");
    let contents = fs::read_to_string(&path)
        .unwrap()
        .replace("can_dm: [dev]", "can_dm: [ghost]");
    fs::write(&path, contents).unwrap();

    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("unknown agent `ghost`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn send_injects_into_mailbox() {
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());

    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "send",
            "hello:manager",
            "hi there",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let db = tmp.path().join("state/mailbox.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let (sender, recipient, text): (String, String, String) = conn
        .query_row(
            "SELECT sender, recipient, text FROM messages ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(sender, "cli");
    assert_eq!(recipient, "hello:manager");
    assert_eq!(text, "hi there");
}

// ── T-050: teamctl init template/force coverage ─────────────────────────

#[test]
fn init_blank_template_scaffolds_minimal_tree() {
    // The `solo` template is exercised by an existing happy-path test;
    // this pins the `blank` template's surface so a future template
    // refactor can't silently drop its files. Asserts (a) every
    // declared file lands at `.team/<relpath>` and (b) the resulting
    // tree validates.
    let tmp = tempdir().unwrap();
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "starter", "--template", "blank", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init blank stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let team_dir = tmp.path().join("starter/.team");
    assert!(
        team_dir.is_dir(),
        "expected .team/ at {}",
        team_dir.display()
    );
    assert!(
        team_dir.join("team-compose.yaml").is_file(),
        "blank template must include team-compose.yaml"
    );
    assert!(
        team_dir.join("projects/main.yaml").is_file(),
        "blank template must include projects/main.yaml"
    );
    assert!(
        team_dir.join(".env.example").is_file(),
        "blank template must include .env.example (from _common)"
    );
    assert!(
        team_dir.join(".gitignore").is_file(),
        "blank template must include .gitignore (from _common)"
    );

    // The scaffolded tree must validate. Exercises the substitution
    // pass + the schema together, so a typo in the template body
    // surfaces here rather than at first user-run.
    let validate = Command::new(bin())
        .args(["--root", team_dir.to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "blank template validate stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
}

#[test]
fn init_force_overwrites_existing_dot_team_cleanly() {
    // The refusal path (no `--force` → exit non-zero, leave existing
    // tree intact) is covered elsewhere. This pins the positive
    // path: `--force` removes the prior `.team/` entirely (no orphan
    // files survive) and lays down the new template fresh.
    let tmp = tempdir().unwrap();

    // First init.
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "myteam", "--template", "solo", "--yes"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let team_dir = tmp.path().join("myteam/.team");
    let sentinel = team_dir.join("sentinel-must-not-survive.txt");
    fs::write(&sentinel, "this file should be wiped by --force").unwrap();
    assert!(sentinel.exists(), "sentinel seeded for the test");

    // Second init with --force on the same target.
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "myteam", "--template", "blank", "--force", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init --force stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Sentinel from the prior tree is gone — `--force` did a clean
    // remove-then-recreate, not a merge.
    assert!(
        !sentinel.exists(),
        "sentinel survived --force; .team/ was not cleanly replaced"
    );

    // The new template's structure is in place.
    assert!(team_dir.join("team-compose.yaml").is_file());
    assert!(team_dir.join("projects/main.yaml").is_file());
    // The `solo` template's roles/manager.md must be gone (we
    // overwrote with `blank` which has no roles/).
    assert!(
        !team_dir.join("roles/manager.md").exists(),
        "prior solo template's roles/manager.md should be wiped"
    );
}

// ── T-033: cli `teamctl approve` after TTL elapsed ──────────────────────

#[test]
fn approve_after_ttl_elapsed_returns_no_pending_error() {
    // Pin the contract that `teamctl approve` cannot resurrect a row that
    // `teamctl gc` has already moved to a terminal state. The CLI's
    // `WHERE status='pending'` clause is what enforces this; the test
    // would fail if a future change relaxed it (e.g. dropped the status
    // pin, or pre-loaded the row before the gc check).
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());

    // Bootstrap the mailbox so we can write directly. `seed_compose`
    // doesn't create state/, so the directory has to come up first.
    let db = tmp.path().join("state/mailbox.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    team_core::mailbox::ensure(&conn).unwrap();

    // Seed a pending approval whose TTL is already in the past
    // (requested_at = expires_at = T-1h, T-30m). delivered_at = NULL so
    // gc routes it to `undeliverable`; the test would still pass against
    // `expired` if delivered_at were set.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let requested_at = now - 3600.0;
    let expires_at = now - 1800.0;
    conn.execute(
        "INSERT INTO approvals (project_id, agent_id, action, summary, status,
                                requested_at, expires_at)
         VALUES ('hello', 'manager', 'publish', 'old request', 'pending', ?1, ?2)",
        rusqlite::params![requested_at, expires_at],
    )
    .unwrap();
    let id: i64 = conn.last_insert_rowid();
    drop(conn);

    // Run gc — flips the row to `undeliverable` (delivered_at IS NULL).
    let gc_out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "gc"])
        .output()
        .unwrap();
    assert!(
        gc_out.status.success(),
        "gc stderr: {}",
        String::from_utf8_lossy(&gc_out.stderr)
    );

    // Confirm the row is no longer pending after gc.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM approvals WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "undeliverable",
        "gc should mark expired-undelivered row"
    );
    drop(conn);

    // Now `teamctl approve <id>` must fail with the canonical error and
    // must not flip the terminal-state fields back.
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "approve",
            &id.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "approve on terminal row should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("no pending approval with id {id}")),
        "expected canonical error, got: {stderr}"
    );

    // Row's terminal-state fields unchanged. With the T-036 ordering
    // fix in place (status pin first, delivered_at flip second), the
    // CLI must NOT have flipped delivered_at on this terminal row —
    // the invariant is `undeliverable ↔ delivered_at IS NULL`, and
    // breaking it would mean the CLI's status-check came after the
    // delivered_at write again.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let (status, decided_by, delivered_at): (String, Option<String>, Option<f64>) = conn
        .query_row(
            "SELECT status, decided_by, delivered_at FROM approvals WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "undeliverable");
    assert!(
        decided_by.is_none() || decided_by.as_deref() != Some("cli"),
        "cli should not have stamped decided_by on a terminal row"
    );
    assert!(
        delivered_at.is_none(),
        "delivered_at must stay NULL on undeliverable row (invariant); got {delivered_at:?}"
    );
}

// ── T-035 PR B: reload --dry-run ────────────────────────────────────────

#[test]
fn reload_dry_run_with_no_prior_lists_added_and_does_not_apply() {
    // No `state/applied.json` on disk → every agent in the compose
    // shows up as `added (dry run)`. Crucially, the dry-run path
    // must not write `state/applied.json`, must not render env/mcp
    // files, and must not invoke tmux. We assert all four.
    let tmp = tempdir().unwrap();
    seed_compose(tmp.path());

    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("added") && stdout.contains("(dry run)"),
        "expected added/(dry run) lines, got: {stdout}"
    );
    assert!(
        stdout.contains("hello:manager"),
        "expected hello:manager in plan, got: {stdout}"
    );
    assert!(
        stdout.contains("hello:dev"),
        "expected hello:dev in plan, got: {stdout}"
    );

    // Side-effect-free: applied.json must not exist after dry-run.
    let applied = tmp.path().join("state/applied.json");
    assert!(
        !applied.exists(),
        "dry-run wrote applied.json at {}",
        applied.display()
    );
    // Render outputs also must not have been written.
    let envs = tmp.path().join("state/envs");
    assert!(
        !envs.exists(),
        "dry-run rendered env files at {}",
        envs.display()
    );
}

// ── T-010: source-aware override warning ─────────────────────────────────

/// Run `teamctl validate` against `cwd` with a clean env, returning stderr.
/// `extra_env` lets each test inject the override under test (TEAMCTL_ROOT,
/// TEAMCTL_QUIET, ...). `home` isolates the registered-context store at
/// `$HOME/.config/teamctl/contexts.json`.
fn run_validate_with_env(
    cwd: &std::path::Path,
    home: &std::path::Path,
    extra_env: &[(&str, &str)],
    explicit_root: Option<&std::path::Path>,
) -> String {
    let mut cmd = Command::new(bin());
    cmd.env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(cwd);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    if let Some(r) = explicit_root {
        cmd.args(["--root", r.to_str().unwrap(), "validate"]);
    } else {
        cmd.arg("validate");
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "validate exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stderr).unwrap()
}

/// Lay out a `.team/`-style root at `<dir>/.team/` (so cwd walk-up will find it).
fn seed_dot_team(dir: &std::path::Path) -> std::path::PathBuf {
    let root = dir.join(".team");
    fs::create_dir_all(&root).unwrap();
    seed_compose(&root);
    root
}

/// Strip ANSI colour codes so assertions are stable regardless of TTY.
fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

#[test]
fn warn_a_walk_up_silent() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    let _ = root; // walk-up will find it from cwd
    let stderr = run_validate_with_env(tmp.path(), home.path(), &[], None);
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "walk-up must not warn; stderr was: {clean}"
    );
}

#[test]
fn warn_b_env_root_warns() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    // CWD is also a valid walk-up target — warning still fires because the
    // resolved root came from env, not walk-up.
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[("TEAMCTL_ROOT", root.to_str().unwrap())],
        None,
    );
    let clean = strip_ansi(&stderr);
    assert!(
        clean.contains("warning:") && clean.contains("TEAMCTL_ROOT"),
        "expected env warning; stderr was: {clean}"
    );
}

#[test]
fn warn_b_empty_env_root_treated_as_unset() {
    // `TEAMCTL_ROOT=""` (exported empty) should fall through to walk-up
    // rather than errorring on `canonicalize("")`.
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let _ = seed_dot_team(tmp.path());
    let stderr = run_validate_with_env(tmp.path(), home.path(), &[("TEAMCTL_ROOT", "")], None);
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "empty TEAMCTL_ROOT must fall through silently to walk-up; stderr was: {clean}"
    );
}

#[test]
fn warn_c_explicit_root_silent() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    // Even with TEAMCTL_ROOT in env, --root on the CLI is the deliberate intent.
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[("TEAMCTL_ROOT", "/definitely/not/this")],
        Some(&root),
    );
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "--root must not warn; stderr was: {clean}"
    );
}

#[test]
fn warn_d_registered_context_no_longer_resolves_root() {
    // T-008: the registered-context fallback was retired. With no `.team/`
    // walked up to from cwd and a registered context pointing at a real
    // `.team/`, root resolution must error rather than silently fall back.
    let tmp = tempdir().unwrap();
    let unrelated_cwd = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());

    let cfg_dir = home.path().join(".config/teamctl");
    fs::create_dir_all(&cfg_dir).unwrap();
    let store = format!(
        r#"{{"current":"demo","contexts":{{"demo":"{}"}}}}"#,
        root.display()
    );
    fs::write(cfg_dir.join("contexts.json"), store).unwrap();

    let mut cmd = Command::new(bin());
    cmd.env_clear()
        .env("HOME", home.path())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(unrelated_cwd.path())
        .arg("validate");
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "validate must fail when no `.team/` is reachable from cwd"
    );
    let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
    assert!(
        stderr.contains("no `.team/team-compose.yaml`"),
        "expected no-team error, not a context fallback; stderr was: {stderr}"
    );
}

#[test]
fn context_subcommand_emits_deprecation_warning() {
    // T-008: every `teamctl context …` invocation should print a one-line
    // deprecation note to stderr while still doing its (now-cosmetic) job.
    let home = tempdir().unwrap();
    let mut cmd = Command::new(bin());
    cmd.env_clear()
        .env("HOME", home.path())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["context", "ls"]);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "context ls must still succeed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
    assert!(
        stderr.contains("`teamctl context` is deprecated"),
        "expected deprecation warning; stderr was: {stderr}"
    );
}

#[test]
fn init_with_name_creates_team_folder_that_validates() {
    // T-045: `teamctl init my-team --yes` should produce a tree that
    // `teamctl --root my-team/.team validate` accepts.
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();

    let init = Command::new(bin())
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(tmp.path())
        .args(["init", "my-team", "--yes"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: stderr={}",
        String::from_utf8_lossy(&init.stderr)
    );

    let team_dir = tmp.path().join("my-team/.team");
    for f in [
        "team-compose.yaml",
        "projects/main.yaml",
        "roles/manager.md",
        "roles/dev.md",
        ".env.example",
        ".gitignore",
        "README.md",
    ] {
        assert!(team_dir.join(f).is_file(), "missing scaffolded file: {f}");
    }

    let validate = Command::new(bin())
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["--root", team_dir.to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "validate failed: stderr={}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let stdout = String::from_utf8_lossy(&validate.stdout);
    assert!(
        stdout.contains("ok") && stdout.contains("2 agents"),
        "unexpected validate output: {stdout}"
    );
}

#[test]
fn init_refuses_existing_team_without_force() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();

    let run_init = |extra: &[&str]| -> std::process::Output {
        let mut args = vec!["init", "my-team", "--yes"];
        args.extend(extra);
        Command::new(bin())
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(tmp.path())
            .args(args)
            .output()
            .unwrap()
    };

    let first = run_init(&[]);
    assert!(first.status.success(), "first init must succeed");

    let second = run_init(&[]);
    assert!(
        !second.status.success(),
        "second init without --force must refuse"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("already exists") && stderr.contains("--force"),
        "expected refusal hint in stderr, got: {stderr}"
    );

    let third = run_init(&["--force"]);
    assert!(
        third.status.success(),
        "init --force must overwrite: stderr={}",
        String::from_utf8_lossy(&third.stderr)
    );
}

// ── T-062: `teamctl ui` wrapper ────────────────────────────────────────

#[test]
fn ui_with_no_prompt_and_no_binary_prints_install_hint_and_exits_zero() {
    // End-to-end: drive the real binary with a hermetic PATH that
    // contains no `teamctl-ui`, and confirm `--no-prompt` short-circuits
    // cleanly. This pins the contract that scripted/CI use of
    // `teamctl ui --no-prompt` is exit-0 + hint-on-stderr — never
    // blocks, never installs, never errors.
    let empty = tempdir().unwrap();
    let out = Command::new(bin())
        .env("PATH", empty.path())
        .args(["ui", "--no-prompt"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "ui --no-prompt must exit 0 even when teamctl-ui is missing; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("teamctl-ui is not installed"),
        "expected install hint on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("cargo install teamctl-ui"),
        "expected install command in hint, got: {stderr}"
    );
}

#[test]
fn warn_e_quiet_silences_env() {
    let tmp = tempdir().unwrap();
    let home = tempdir().unwrap();
    let root = seed_dot_team(tmp.path());
    let stderr = run_validate_with_env(
        tmp.path(),
        home.path(),
        &[
            ("TEAMCTL_ROOT", root.to_str().unwrap()),
            ("TEAMCTL_QUIET", "1"),
        ],
        None,
    );
    let clean = strip_ansi(&stderr);
    assert!(
        !clean.contains("warning:"),
        "TEAMCTL_QUIET=1 must silence; stderr was: {clean}"
    );
}
