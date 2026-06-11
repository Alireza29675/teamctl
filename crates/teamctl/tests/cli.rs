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
    model: claude-opus-4-8
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
    assert!(stdout.contains("2 agent sessions"), "got: {stdout}");
}

// ── T-325: init Stage 4 emits cascading role_prompt list form ───────────
// The init skill writes role_prompt: as a YAML list. Managers cascade
// `[_base.md, <name>.md]`; workers cascade
// `[_base.md, _worker.md, <name>.md]` (asymmetric tier-shape by owner
// direction, matching the merged #295 dogfood convention). The render
// layer already concats list form at boot (covered in team-core tests);
// this test pins the CLI-level contract — validate accepts what the
// skill emits, so the skill's emission round-trips through the rest of
// the stack without schema regression.
#[test]
fn validate_accepts_cascading_role_prompt_list_form() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("team-compose.yaml"),
        r#"
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
supervisor:
  type: tmux
  tmux_prefix: cascade-
projects:
  - file: projects/cascade.yaml
"#,
    )
    .unwrap();
    fs::create_dir_all(tmp.path().join("projects")).unwrap();
    fs::write(
        tmp.path().join("projects/cascade.yaml"),
        r#"
version: 2
project:
  id: cascade
  name: Cascade
  cwd: .
channels:
  - name: all
    members: "*"
managers:
  pm:
    runtime: claude-code
    model: claude-opus-4-8
    role_prompt:
      - roles/_base.md
      - roles/pm.md
    can_dm: [eng]
    can_broadcast: [all]
workers:
  eng:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: pm
    role_prompt:
      - roles/_base.md
      - roles/_worker.md
      - roles/eng.md
    can_dm: [pm]
    can_broadcast: [all]
"#,
    )
    .unwrap();

    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "validate should accept cascade list-form role_prompt; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("1 project"), "got: {stdout}");
    assert!(stdout.contains("2 agent sessions"), "got: {stdout}");
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

// ── T-091: --help surfaces the version ──────────────────────────────────

#[test]
fn help_header_includes_version_from_cargo_pkg_version() {
    // The clap `version` attribute on the top-level Cli derive plus a
    // help_template that includes `{name} {version}` puts the version
    // line at the top of `teamctl --help`. Pinning it here so a future
    // template edit can't silently drop the line; the assertion uses
    // CARGO_PKG_VERSION so it tracks the crate version automatically.
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let expected = format!("teamctl {}", env!("CARGO_PKG_VERSION"));
    assert!(
        stdout.contains(&expected),
        "expected `{expected}` in --help output; got:\n{stdout}"
    );
}

#[test]
fn version_flag_still_prints_version() {
    // Companion to the help-header test: `--version` keeps working and
    // produces the same string the help header surfaces.
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected version in --version output; got: {stdout}"
    );
}

// ── T-050: teamctl init template/force coverage ─────────────────────────

#[test]
fn init_blank_template_scaffolds_minimal_tree() {
    // Pins the `blank` template's surface so a future template
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

    // Blank scaffolds no agents, so `lead_attach_target` → None and the
    // `Next:` block must omit the attach/bot hints. Guards the agentless
    // branch end-to-end (complements the agent-bearing ideate assertion).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("teamctl attach"),
        "agentless blank must not suggest attach; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("teamctl bot setup"),
        "agentless blank must not suggest bot setup; got:\n{stdout}"
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
fn init_essentials_template_scaffolds_two_project_tree() {
    // T-206: `essentials` ships a two-project layout — blank `main`
    // for the operator + `ops` with the `ops` agent. Pins the
    // file shape so a future template refactor can't silently drop
    // any of the seven files, and asserts the tree validates so a
    // typo in the ops agent's compose surfaces here.
    let tmp = tempdir().unwrap();
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "starter", "--template", "essentials", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init essentials stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let team_dir = tmp.path().join("starter/.team");
    for relpath in [
        "team-compose.yaml",
        "projects/main.yaml",
        "projects/ops.yaml",
        "roles/ops.md",
        ".env.example",
        ".gitignore",
        "README.md",
    ] {
        assert!(
            team_dir.join(relpath).is_file(),
            "essentials template must include {relpath}"
        );
    }

    let validate = Command::new(bin())
        .args(["--root", team_dir.to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "essentials template validate stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
}

#[test]
fn init_ideate_and_build_template_scaffolds_with_subagents() {
    // T-382: the `ideate-and-build` template is the flagship showcase —
    // its role prompts reference a stable of sub-agents that must
    // actually ship. Pin the file shape so a template refactor can't
    // silently drop a sub-agent markdown (which would leave the roles
    // pointing at nothing), and assert the tree validates.
    let tmp = tempdir().unwrap();
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "studio", "--template", "ideate-and-build", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init ideate-and-build stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The hero-path `Next:` block must wire the derived lead manager into
    // the attach hint end-to-end. `init` derives it from the just-written
    // compose on disk — a different parse path than the `lead_attach_target`
    // unit tests exercise. ideate-and-build's workers report to `executor`,
    // so the hint is `teamctl attach main:executor`, not the back-channel
    // `compass`; an agent-bearing template also offers `teamctl bot setup`.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("teamctl attach main:executor"),
        "Next: block must suggest attaching to the lead manager; got:\n{stdout}"
    );
    assert!(
        stdout.contains("teamctl bot setup"),
        "Next: block must offer bot setup for an agent-bearing template; got:\n{stdout}"
    );

    let team_dir = tmp.path().join("studio/.team");
    for relpath in [
        "team-compose.yaml",
        "projects/main.yaml",
        "charter.md",
        "subagents/code-investigator.md",
        "subagents/implementer.md",
        "subagents/test-author.md",
        "subagents/qa-tester.md",
        "subagents/pr-narrator.md",
        "subagents/code-roaster.md",
        "subagents/memory-writer.md",
        "subagents/product-researcher.md",
        "subagents/feasibility-analyst.md",
        "subagents/deep-research.md",
        "subagents/learn.md",
        "subagents/pr-summarizer.md",
        "subagents/ideator.md",
        "subagents/code-review.md",
        "subagents/security-review.md",
    ] {
        assert!(
            team_dir.join(relpath).is_file(),
            "ideate-and-build template must include {relpath}"
        );
    }

    let validate = Command::new(bin())
        .args(["--root", team_dir.to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "ideate-and-build template validate stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
}

#[test]
fn ideate_and_build_template_renders_per_agent_subagents() {
    // T-382: the `subagents:` declared in the template compose must
    // resolve through the real render path into Claude Code's `--agents`
    // JSON — not just exist as files. Load the shipped template compose
    // straight from the crate assets and assert each agent gets exactly
    // its declared stable (and the executor, which declares none, gets
    // nothing). This is the authoritative check that the template's
    // role-prompt sub-agent references are backed by real config.
    let template =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/templates/ideate-and-build");
    let compose = team_core::compose::Compose::load(&template).unwrap();

    let expect_names = |agent: &str, expected: &[&str]| {
        let h = compose
            .agents()
            .find(|h| h.agent == agent)
            .unwrap_or_else(|| panic!("agent `{agent}` not found in template compose"));
        let json = team_core::render::render_subagents(&compose, h)
            .unwrap()
            .unwrap_or_else(|| panic!("agent `{agent}` rendered no sub-agents"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = v.as_object().expect("agents json is an object");
        let mut got: Vec<&str> = obj.keys().map(String::as_str).collect();
        got.sort_unstable();
        let mut want = expected.to_vec();
        want.sort_unstable();
        assert_eq!(got, want, "sub-agent set for `{agent}`");
    };

    let engineer_stable = [
        "code-investigator",
        "implementer",
        "test-author",
        "qa-tester",
        "pr-narrator",
        "code-roaster",
        "code-review",
        "security-review",
    ];
    expect_names("engineer_1", &engineer_stable);
    expect_names("engineer_2", &engineer_stable);
    expect_names(
        "compass",
        &[
            "memory-writer",
            "code-investigator",
            "product-researcher",
            "feasibility-analyst",
            "deep-research",
            "learn",
            "ideator",
        ],
    );
    // The Executor gets a single sub-agent: pr-summarizer, so it can turn
    // an engineer's ready PR into a plain-language summary for the operator.
    expect_names("executor", &["pr-summarizer"]);
}

#[test]
fn init_yes_without_template_defaults_to_essentials() {
    // T-206: the non-interactive default changed from `solo` to
    // `essentials`. Pin the contract — `--yes` with no `--template`
    // must land the operator on the `essentials` two-project shape,
    // not the bare blank tree.
    let tmp = tempdir().unwrap();
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "starter", "--yes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init --yes (no template) stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let team_dir = tmp.path().join("starter/.team");
    // The `ops` project file + `ops` role are essentials-only
    // markers; their presence proves the default landed on
    // essentials, not blank.
    assert!(
        team_dir.join("projects/ops.yaml").is_file(),
        "--yes default must land essentials (projects/ops.yaml missing)"
    );
    assert!(
        team_dir.join("roles/ops.md").is_file(),
        "--yes default must land essentials (roles/ops.md missing)"
    );
}

#[test]
fn init_template_guided_with_yes_errors() {
    // T-206: `guided` execs `claude /teamctl:init` after an
    // interactive confirm-intent prompt, so it can't run under
    // `--yes`. The CLI rejects the combo up front with a clear
    // message rather than silently flipping to a different template.
    let tmp = tempdir().unwrap();
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "starter", "--template", "guided", "--yes"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "`--template guided --yes` must exit non-zero; stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("guided") && stderr.contains("interactive"),
        "expected error to name `guided` + `interactive`; got: {stderr}"
    );
    // And no `.team/` directory was created — the failure path
    // doesn't leave a partial scaffold behind.
    assert!(
        !tmp.path().join("starter/.team").exists(),
        "rejected --template guided --yes must not create a .team/ tree"
    );
}

#[test]
fn init_force_overwrites_existing_dot_team_cleanly() {
    // The refusal path (no `--force` → exit non-zero, leave existing
    // tree intact) is covered elsewhere. This pins the positive
    // path: `--force` removes the prior `.team/` entirely (no orphan
    // files survive) and lays down the new template fresh.
    let tmp = tempdir().unwrap();

    // First init — seed with `essentials` so we have a richer tree
    // (the `roles/ops.md` marker doubles as the wipe-check).
    let out = Command::new(bin())
        .current_dir(tmp.path())
        .args(["init", "myteam", "--template", "essentials", "--yes"])
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
    // The prior `essentials` template's roles/ops.md must be
    // gone — `blank` has no roles/ so a stale file there would prove
    // --force merged rather than replaced.
    assert!(
        !team_dir.join("roles/ops.md").exists(),
        "prior essentials template's roles/ops.md should be wiped"
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

// ── T-133: scoped <project-name> arg on up/down/reload ──────────────────

fn seed_two_projects(root: &std::path::Path) {
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
  - file: projects/alpha.yaml
  - file: projects/beta.yaml
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::write(
        root.join("projects/alpha.yaml"),
        r#"
version: 2
project:
  id: alpha
  name: Alpha
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
    can_dm: [dev]
workers:
  dev:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
    can_dm: [manager]
"#,
    )
    .unwrap();
    fs::write(
        root.join("projects/beta.yaml"),
        r#"
version: 2
project:
  id: beta
  name: Beta
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
    can_dm: [dev]
workers:
  dev:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
    can_dm: [manager]
"#,
    )
    .unwrap();
}

#[test]
fn reload_dry_run_with_unknown_project_lists_known_and_exits_nonzero() {
    // Resolution miss: error names the rejected input AND lists every
    // available project.id so the operator can copy-paste a fix.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "ghost",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected nonzero exit");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("ghost"), "names rejected input: {stderr}");
    assert!(stderr.contains("alpha"), "lists alpha: {stderr}");
    assert!(stderr.contains("beta"), "lists beta: {stderr}");
}

#[test]
fn reload_dry_run_scoped_to_project_lists_only_that_project() {
    // The plan covers the whole compose, but the scoped run filters
    // it down to the named project. alpha's two agents land as
    // `added`; beta's never appear in the output.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("alpha:manager"), "got: {stdout}");
    assert!(stdout.contains("alpha:dev"), "got: {stdout}");
    assert!(
        !stdout.contains("beta:manager"),
        "scoped plan must omit beta: {stdout}"
    );
    assert!(
        !stdout.contains("beta:dev"),
        "scoped plan must omit beta: {stdout}"
    );
}

#[test]
fn reload_dry_run_resolves_project_by_filename_stem() {
    // Filename fallback: operator types `alpha` (matches both file
    // stem and project.id here, but the filename path is exercised
    // when project.id differs from the stem — covered in unit
    // tests). Pinning the integration round-trip so the resolver is
    // wired into the reload subcommand.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "beta",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("beta:manager"), "got: {stdout}");
    assert!(!stdout.contains("alpha:"), "got: {stdout}");
}

#[test]
fn reload_no_arg_unchanged_lists_every_project() {
    // Back-compat pin: omitting the arg keeps the original behavior
    // — every agent across every project shows in the plan.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("alpha:manager"), "got: {stdout}");
    assert!(stdout.contains("beta:manager"), "got: {stdout}");
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
    // T-045 / T-206: `teamctl init my-team --yes` should produce a
    // tree that `teamctl --root my-team/.team validate` accepts.
    // Default template under `--yes` is `essentials` post-T-206 —
    // two projects (`main` + `ops`) with a single `ops` agent.
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
        "projects/ops.yaml",
        "roles/ops.md",
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
    // Essentials ships 2 projects (`main` + `ops`) and 1 agent
    // (`ops`). The validate summary surfaces both counts.
    assert!(
        stdout.contains("ok") && stdout.contains("2 projects") && stdout.contains("1 agent"),
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

// ── T-241: `teamctl adjust` wrapper ────────────────────────────────────

#[test]
fn adjust_yes_flag_errors_clearly_with_actionable_message() {
    // T-241 DoD: `teamctl adjust --yes` must reject early with a
    // clear, actionable error. Pinned end-to-end through the real
    // binary so the clap surface + the runtime check are both
    // exercised. Hermetic PATH so a stray `claude` on the dev box
    // can't accidentally make this test pass by exec'ing instead of
    // erroring.
    let empty = tempdir().unwrap();
    let out = Command::new(bin())
        .env("PATH", empty.path())
        .args(["adjust", "--yes"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "adjust --yes must exit non-zero; stderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("interactive-only"),
        "expected `interactive-only` in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("--yes"),
        "expected `--yes` in stderr, got: {stderr}"
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

// ── T-305: per-agent scope on up/down/reload ────────────────────────────
//
// `seed_two_projects` gives alpha {manager, dev} + beta {manager, dev}.
// These exercise the CLI surface + selector resolution without tmux:
// `reload --dry-run` for the force path, and the error/usage paths
// (which short-circuit before the supervisor is ever touched).

#[test]
fn reload_dry_run_scoped_to_agent_forces_only_that_agent() {
    // `reload <project> <agent>` force-restarts exactly that agent —
    // the line is `(forced)`, not the diff path's `added`/`changed`.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
            "dev",
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
        stdout.contains("reloaded · alpha:dev (forced) (dry run)"),
        "force line for the scoped agent: {stdout}"
    );
    assert!(
        !stdout.contains("alpha:manager"),
        "must not touch the unscoped sibling: {stdout}"
    );
    assert!(
        !stdout.contains("beta:"),
        "must not touch other projects: {stdout}"
    );
}

#[test]
fn reload_dry_run_except_agent_forces_the_complement() {
    // `reload <project> --except <agent>` force-restarts every agent
    // in the project EXCEPT the named one.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
            "--except",
            "dev",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("reloaded · alpha:manager (forced) (dry run)"),
        "complement is forced: {stdout}"
    );
    assert!(
        !stdout.contains("alpha:dev"),
        "the excepted agent is left alone: {stdout}"
    );
    assert!(
        !stdout.contains("beta:"),
        "other projects untouched: {stdout}"
    );
}

#[test]
fn reload_project_only_keeps_the_unchanged_diff_path() {
    // Back-compat pin: `<project>`-only reload (no agent selector) is
    // the existing diff path — agents land as `added` (no prior
    // snapshot), never force-restarted.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("added   · alpha:manager"), "got: {stdout}");
    assert!(
        !stdout.contains("(forced)"),
        "project-only must not force-restart: {stdout}"
    );
}

#[test]
fn scoped_unknown_agent_errors_listing_valid_names() {
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
            "ghost",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown agent must exit nonzero");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("ghost"), "names the bad input: {stderr}");
    assert!(stderr.contains("manager"), "lists valid agents: {stderr}");
    assert!(stderr.contains("dev"), "lists valid agents: {stderr}");
}

#[test]
fn except_unknown_agent_also_errors() {
    // A typo'd exclusion fails loudly rather than silently acting on
    // every agent.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
            "--except",
            "ghost",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("ghost"), "names the bad input: {stderr}");
}

#[test]
fn positional_list_and_except_are_mutually_exclusive() {
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "down",
            "alpha",
            "dev",
            "--except",
            "manager",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "supplying both forms must be a usage error"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("cannot be used with"),
        "clap conflict message: {stderr}"
    );
}

#[test]
fn except_requires_a_project() {
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "down",
            "--except",
            "dev",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "--except without a project is invalid"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("PROJECT"),
        "clap names the missing required arg: {stderr}"
    );
}

#[test]
fn except_every_agent_reports_empty_scope() {
    // Excepting everyone resolves to "act on nothing" — a graceful
    // no-op line, not an error.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "alpha",
            "--except",
            "manager",
            "--except",
            "dev",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("no agents in scope for project alpha"),
        "got: {stdout}"
    );
}

// ── T-384: reload --force ───────────────────────────────────────────────
//
// The forced-keep promotion and the `reloaded · id (forced)` output are
// unit-tested in reload.rs / snapshot.rs: a `keep` state needs a prior
// `state/applied.json` whose fingerprints match the live compose, which
// the no-tmux integration harness can't produce (it never runs `up`).
// These pin the CLI surface instead — the flag parses, composes with
// `--fresh` / `--dry-run`, and on a fresh root (no prior snapshot, every
// agent is `add`) `--force` has no kept agents to promote, so it stays a
// clean no-op on the diff path.

#[test]
fn reload_force_dry_run_without_prior_lists_added_not_forced() {
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "--force",
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
        stdout.contains("added   · alpha:manager"),
        "fresh root → diff path lists adds: {stdout}"
    );
    assert!(
        !stdout.contains("(forced)"),
        "no prior snapshot → no kept agents to force-restart: {stdout}"
    );
}

#[test]
fn reload_force_composes_with_fresh_and_dry_run() {
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args([
            "--root",
            tmp.path().to_str().unwrap(),
            "reload",
            "--dry-run",
            "--fresh",
            "--force",
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
        stdout.contains("(fresh)") && stdout.contains("(dry run)"),
        "--force composes with --fresh and --dry-run: {stdout}"
    );
}

#[test]
fn reload_help_documents_force_flag() {
    let out = Command::new(bin())
        .args(["reload", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success(), "reload --help failed");
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(
        help.contains("--force"),
        "reload --help must document --force: {help}"
    );
}

#[test]
fn help_documents_all_scope_levels_consistently() {
    // Acceptance criterion: `--help` for each of up/down/reload
    // documents no-arg = all projects, <project>, <project> <agent>…,
    // and <project> --except <agent>….
    for cmd in ["up", "down", "reload"] {
        let out = Command::new(bin()).args([cmd, "--help"]).output().unwrap();
        assert!(out.status.success(), "{cmd} --help failed");
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(
            help.contains("every project"),
            "{cmd} --help missing all-projects form: {help}"
        );
        assert!(
            help.contains("every agent in it"),
            "{cmd} --help missing project-only form: {help}"
        );
        assert!(
            help.contains("just those agents"),
            "{cmd} --help missing positional-list form: {help}"
        );
        assert!(
            help.contains("--except"),
            "{cmd} --help missing --except form: {help}"
        );
    }
}

// ── T-310: id-charset validation — qa PoC class end-to-end ──────────────
//
// A `project.id` (or agent id) containing shell-metacharacter content
// must be rejected by `teamctl validate` AND must bail before
// `build_up_command` ever runs on `teamctl up` / `reload` / `down`.
// The unit tests in `team-core::validate` pin the validator itself;
// these integration tests pin the criterion-2 wiring: the supervisor
// is never reached when a compose carries a bad id.

fn seed_compose_with_evil_project_id(root: &std::path::Path) {
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
  - file: projects/evil.yaml
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    // qa PoC class: shell-metacharacter content in `project.id`.
    fs::write(
        root.join("projects/evil.yaml"),
        r#"
version: 2
project:
  id: "evil; touch /tmp/teamctl-t310-pwn"
  name: Evil
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
workers:
  dev:
    runtime: claude-code
    model: claude-sonnet-4-6
    reports_to: manager
"#,
    )
    .unwrap();
}

#[test]
fn validate_rejects_project_id_with_shell_metacharacters() {
    // `teamctl validate` exit-nonzero with a clear, actionable error
    // naming the rejected id (acceptance criterion 1).
    let tmp = tempdir().unwrap();
    seed_compose_with_evil_project_id(tmp.path());
    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "must exit nonzero");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("evil; touch /tmp/teamctl-t310-pwn"),
        "names the rejected id: {stderr}"
    );
    assert!(
        stderr.contains("disallowed characters"),
        "explains the rule: {stderr}"
    );
}

#[test]
fn down_bails_on_evil_project_id_before_reaching_supervisor() {
    // Criterion 2: a shell-meta `project.id` can no longer reach the
    // shell via `build_up_command` on `teamctl down`. Pre-T-310 this
    // command was NOT validate-gated, so a malicious compose flowed
    // unquoted into `sh -c`. Now `down` validates + bails first.
    //
    // The negative existence check is the security pin: the injected
    // `touch /tmp/teamctl-t310-pwn` must NOT have run.
    let pwn_marker = std::path::PathBuf::from("/tmp/teamctl-t310-pwn");
    let _ = std::fs::remove_file(&pwn_marker);

    let tmp = tempdir().unwrap();
    seed_compose_with_evil_project_id(tmp.path());
    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "down"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "down must bail on validation error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("disallowed characters"),
        "down must surface the id-charset error: {stderr}"
    );
    assert!(
        !pwn_marker.exists(),
        "INJECTION FIRED — `build_up_command` was reached: marker {} exists",
        pwn_marker.display()
    );
    // Best-effort cleanup if a regression ever creates it.
    let _ = std::fs::remove_file(&pwn_marker);
}

#[test]
fn validate_accepts_existing_conformant_id_shapes() {
    // Acceptance criterion 5 (no regression for conformant teams):
    // the existing `seed_two_projects` shape passes validate cleanly.
    let tmp = tempdir().unwrap();
    seed_two_projects(tmp.path());
    let out = Command::new(bin())
        .args(["--root", tmp.path().to_str().unwrap(), "validate"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "conformant ids must validate: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
