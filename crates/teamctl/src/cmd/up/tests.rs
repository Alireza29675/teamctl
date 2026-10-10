use super::DEFAULT_BOOT_SCRIPT;
use super::DEFAULT_WRAPPER;
use super::*;
use std::collections::BTreeMap;
use team_core::compose::*;
use team_core::render::role_prompt_concat_path;

#[test]
fn registry_entries_groups_by_project_and_sorts_agents() {
    let root = Path::new("/r/a/.team");
    let agents = vec![("main", "scout"), ("main", "compass"), ("ops", "otto")];
    let entries = registry_entries(agents.into_iter(), root, "t-", None, "2026-06-13T00:00:00Z");
    assert_eq!(entries.len(), 2);
    // BTreeMap keys → project-sorted; agents sorted within a project.
    assert_eq!(entries[0].project_id, "main");
    assert_eq!(entries[0].agents, vec!["compass", "scout"]);
    assert_eq!(entries[0].root, root);
    assert_eq!(entries[0].tmux_prefix, "t-");
    assert_eq!(entries[0].started_at, "2026-06-13T00:00:00Z");
    assert_eq!(entries[1].project_id, "ops");
    assert_eq!(entries[1].agents, vec!["otto"]);
}

#[test]
fn registry_entries_scoped_records_only_that_project() {
    let agents = vec![("main", "compass"), ("ops", "otto")];
    let entries = registry_entries(
        agents.into_iter(),
        Path::new("/r/a/.team"),
        "t-",
        Some("ops"),
        "T0",
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].project_id, "ops");
    assert_eq!(entries[0].agents, vec!["otto"]);
}

#[test]
fn up_guards_against_same_name_in_a_different_folder() {
    use team_core::registry::{Registry, TeamEntry};
    // A compose for project `main`, loaded from a tempdir (its canonical
    // root) — the self-contained-fixture pattern (no shipped template).
    let team = tempfile::tempdir().unwrap();
    fs::create_dir_all(team.path().join("projects")).unwrap();
    fs::write(
            team.path().join("team-compose.yaml"),
            "version: 2\nsupervisor:\n  type: tmux\n  tmux_prefix: t-\nprojects:\n  - file: projects/main.yaml\n",
        )
        .unwrap();
    fs::write(
            team.path().join("projects/main.yaml"),
            "version: 2\nproject:\n  id: main\n  name: Main\n  cwd: ./workspace\nmanagers:\n  lead:\n    runtime: claude-code\n    role_prompt: roles/lead.md\n",
        )
        .unwrap();
    let compose = Compose::load(team.path()).unwrap();

    let make_reg = |root: PathBuf| {
        let mut reg = Registry::default();
        reg.teams.push(TeamEntry {
            project_id: "main".into(),
            root,
            tmux_prefix: "t-".into(),
            agents: vec!["lead".into()],
            sessions: vec![],
            started_at: "T0".into(),
        });
        reg
    };
    // `main` already up at a DIFFERENT, live folder → bail, naming it.
    let other = make_reg(PathBuf::from("/elsewhere/.team"));
    let live = |p: &Path| p == Path::new("/elsewhere/.team/team-compose.yaml");
    let err = guard_no_name_collision_in(&other, &compose, None, &live)
        .expect_err("same name in a different live folder must be refused");
    assert!(
        err.to_string().contains("/elsewhere/.team"),
        "error names the conflicting dir: {err}"
    );
    assert!(
        err.to_string().contains("teamctl down"),
        "error gives actionable resolution: {err}"
    );
    // A re-up of the SAME (project, root) → not a conflict.
    let same = make_reg(compose.root.clone());
    assert!(guard_no_name_collision_in(&same, &compose, None, &live).is_ok());
}

#[test]
fn record_then_clear_roundtrips_a_real_compose() {
    // End-to-end through the actual `up`/`down` glue (Compose::load →
    // compose.agents() → registry_entries → upsert_many → teams.json,
    // then clear) — the closest to the up→down round trip without
    // spawning tmux. The compose is a self-contained fixture written
    // to a tempdir, NOT a shipped template: templates carry
    // `{{project_id}}` placeholders that don't parse before `init`
    // substitution (T-466 regression — a real one broke this test once
    // a template was templatized). The config dir is a second tempdir,
    // so no `$HOME` touch and no env race.
    let team = tempfile::tempdir().unwrap();
    fs::create_dir_all(team.path().join("projects")).unwrap();
    fs::write(
        team.path().join("team-compose.yaml"),
        "version: 2\n\
             supervisor:\n  type: tmux\n  tmux_prefix: treg-\n\
             projects:\n  - file: projects/demo.yaml\n",
    )
    .unwrap();
    fs::write(
            team.path().join("projects/demo.yaml"),
            "version: 2\n\
             project:\n  id: demo\n  name: Demo\n  cwd: ./workspace\n\
             managers:\n  lead:\n    runtime: claude-code\n    role_prompt: roles/lead.md\n\
             workers:\n  helper:\n    runtime: claude-code\n    role_prompt: roles/helper.md\n    reports_to: lead\n",
        )
        .unwrap();
    let compose = Compose::load(team.path()).unwrap();
    let cfg = tempfile::tempdir().unwrap();

    record_in_registry_in(cfg.path(), &compose, None);

    let reg = team_core::registry::load(cfg.path()).unwrap();
    assert_eq!(reg.teams.len(), 1, "one row for the single project");
    let t = &reg.teams[0];
    assert_eq!(t.project_id, "demo");
    assert_eq!(t.root, compose.root, "row keyed on the compose root");
    assert_eq!(t.tmux_prefix, compose.global.supervisor.tmux_prefix);
    assert_eq!(t.agents, vec!["helper", "lead"], "roster folded + sorted");
    assert!(!t.started_at.is_empty(), "started_at stamped");

    // The `down` clear path empties this root.
    team_core::registry::clear(cfg.path(), &compose.root, None).unwrap();
    let after = team_core::registry::load(cfg.path()).unwrap();
    assert!(
        after.teams.is_empty(),
        "whole-root clear empties the registry"
    );
}

/// The wrapper's `auto_confirm_known_dialogs` watcher relies on a
/// fixed set of dialog-header substrings. A silent edit that drops
/// one of them would re-strand agents at boot or mid-shift, so
/// pin them here.
#[test]
fn freshen_action_gates_on_fresh_and_runtime() {
    // Not fresh → nothing, regardless of runtime. Fresh → move
    // Claude's session aside, move codex's per-agent sessions dir
    // aside, move opencode's per-agent session db aside; gemini
    // stays a warn-and-skip (parity gap), never an abort.
    assert_eq!(freshen_action("claude-code", false), FreshenAction::Skip);
    assert_eq!(freshen_action("codex", false), FreshenAction::Skip);
    assert_eq!(freshen_action("opencode", false), FreshenAction::Skip);
    assert_eq!(freshen_action("claude-code", true), FreshenAction::Freshen);
    assert_eq!(
        freshen_action("codex", true),
        FreshenAction::WipeCodexSessions
    );
    assert_eq!(
        freshen_action("opencode", true),
        FreshenAction::WipeOpencodeSessions
    );
    assert_eq!(
        freshen_action("gemini", true),
        FreshenAction::UnsupportedRuntime
    );
}

/// The auto-confirm watcher dismisses the one-shot dialogs that would
/// otherwise strand a headless pane. `Quick safety check:` is the
/// first-run trust-folder prompt that `--permission-mode auto` (the
/// headless default since 0.8.7) raises — the watcher didn't match it,
/// so an auto session froze at boot whenever the pre-trust missed. It
/// is a one-time trust gate, not `auto`'s risky-action classifier, so
/// accepting it keeps the safety gate intact; the watcher must never
/// match `auto`'s risky-action prompts. Because the header is ordinary
/// prose, the watcher requires it to co-occur with the menu line
/// `trust this folder` before sending Enter — pin both so a future edit
/// can't drop the co-occurrence guard and reintroduce stray Enters. The
/// MCP-enable dialog is auto-accepted the same way (Enter enables the
/// discovered project MCP servers), gated on its own two-string
/// co-occurrence so prose can't trip it.
#[test]
fn wrapper_auto_confirm_patterns_present() {
    for marker in [
        "Loading development channels",
        "Bypass Permissions mode",
        "Stop and wait for limit to reset",
        "Quick safety check:",
        "MCP servers may execute code",
        "auto_confirm_known_dialogs",
    ] {
        assert!(
            DEFAULT_WRAPPER.contains(marker),
            "DEFAULT_WRAPPER missing marker: {marker}",
        );
    }
    // The trust-folder and MCP-enable dialogs must each be gated on a
    // two-string co-occurrence, not a bare match: both greps per dialog
    // must be present in the watcher.
    assert!(
        DEFAULT_WRAPPER.contains("grep -q 'Quick safety check:'")
            && DEFAULT_WRAPPER.contains("grep -q 'trust this folder'"),
        "watcher must require 'Quick safety check:' AND 'trust this folder' to co-occur",
    );
    assert!(
        DEFAULT_WRAPPER.contains("grep -q 'MCP servers may execute code'")
            && DEFAULT_WRAPPER.contains("grep -q 'Enter to confirm · Esc'"),
        "watcher must require 'MCP servers may execute code' AND the 'Enter to confirm · Esc' \
             footer chrome to co-occur",
    );
}

/// Owner-reported crash-loop (2026-06-13): two installs that share a
/// `project.id` derive the same deterministic session UUID; the
/// resume-probe globs every cwd slug and matches the *other* install's
/// jsonl, and claude — scoped to this cwd — can't resume it, exiting
/// non-zero forever. The wrapper self-heals: after a `--resume` launch
/// exits non-zero it retries ONCE, forcing a cwd-scoped `--session-id`
/// (per-cwd, verified) so it opens a fresh local session on a collision
/// and errors harmlessly on a genuine crash. Pin the load-bearing pieces
/// so a future wrapper edit can't silently drop the heal and bring the
/// crash-loop back.
#[test]
fn wrapper_self_heals_resume_collision() {
    // The --resume branch must tag the launch so the heal can tell a
    // resume from a fresh launch.
    assert!(
        DEFAULT_WRAPPER.contains("--resume \"$CLAUDE_SESSION_ID\"")
            && DEFAULT_WRAPPER.contains("RESUMED=1"),
        "wrapper must mark RESUMED=1 when launching with --resume",
    );
    // The one-shot fallback forces a fresh cwd-scoped --session-id.
    assert!(
        DEFAULT_WRAPPER.contains("[ \"${FORCE_FRESH_SESSION:-0}\" = 1 ]")
            && DEFAULT_WRAPPER.contains("--session-id \"$CLAUDE_SESSION_ID\""),
        "wrapper must force a fresh --session-id launch when FORCE_FRESH_SESSION is set",
    );
    // The trigger: a resumed launch that exits non-zero retries exactly
    // once (guarded by RESUMED and the one-shot flag, so it can't loop).
    assert!(
        DEFAULT_WRAPPER.contains("[ \"$ec\" -ne 0 ]")
            && DEFAULT_WRAPPER.contains("[ \"${RESUMED:-0}\" = 1 ]")
            && DEFAULT_WRAPPER.contains("FORCE_FRESH_SESSION=1"),
        "wrapper must retry-fresh only after a resumed launch exits non-zero",
    );
    // Order is load-bearing and a plain substring pin can't catch a
    // reorder: the retry must arm the one-shot and `continue` BEFORE the
    // reset clears it. If a future edit moved `continue` below the reset
    // (defeating the heal) or dropped the reset (risking a loop), this
    // contiguous block changes and the test fails.
    assert!(
        DEFAULT_WRAPPER.contains(
            "        FORCE_FRESH_SESSION=1\n        continue\n    fi\n    FORCE_FRESH_SESSION=0\n"
        ),
        "the retry must set the one-shot and `continue` before the reset clears it",
    );
    // Per-iteration reset of RESUMED guards a stale resume flag from
    // carrying into a later iteration and mis-arming the heal.
    assert!(
        DEFAULT_WRAPPER.contains("RESUMED=0"),
        "wrapper must reset RESUMED each iteration",
    );
}

/// #383 Phase 3a: the wrapper threads per-agent sub-agents via
/// `--agents` when render wrote the JSON file (guarded by `[ -f ]`, so
/// agents with no `subagents:` pass no flag). Pin the marker so a silent
/// wrapper edit can't drop it and strand declared sub-agents.
#[test]
fn wrapper_threads_subagents_via_agents_flag() {
    assert!(
        DEFAULT_WRAPPER.contains("--agents \"$(cat \"$CLAUDE_AGENTS_JSON\")\""),
        "wrapper must pass --agents from CLAUDE_AGENTS_JSON",
    );
}

/// #383 Phase 3b: the wrapper threads per-agent skills via `--add-dir`
/// when render materialized the scope dir (guarded by `[ -d ]`, so
/// agents with no `skills:` pass no flag). Pin the marker so a silent
/// wrapper edit can't drop it and strand declared skills.
#[test]
fn wrapper_threads_skills_via_add_dir_flag() {
    assert!(
        DEFAULT_WRAPPER.contains("--add-dir \"$CLAUDE_AGENT_SCOPE\""),
        "wrapper must pass --add-dir from CLAUDE_AGENT_SCOPE",
    );
}

/// T-361: headless claude-code agents default to `--permission-mode
/// auto` and no longer pass `--dangerously-skip-permissions`. The
/// attended opt-out keys off PERMISSION_MODE, which `render()` omits for
/// agents with no `permission_mode:` — so under `set -u` the comparison
/// must default the var. Pin the new shape so a silent edit can't bring
/// back the bypass-everything flag, drop the auto default, or break the
/// unset-safe attended branch.
#[test]
fn wrapper_defaults_headless_to_permission_mode_auto() {
    assert!(
        DEFAULT_WRAPPER.contains("--permission-mode \"${PERMISSION_MODE:-auto}\""),
        "wrapper must default headless agents to --permission-mode auto",
    );
    assert!(
        !DEFAULT_WRAPPER.contains("--dangerously-skip-permissions"),
        "wrapper must not pass --dangerously-skip-permissions (#361)",
    );
    assert!(
        DEFAULT_WRAPPER.contains("[ \"${PERMISSION_MODE:-}\" = \"attended\" ]"),
        "wrapper must keep the set -u-safe attended opt-out branch",
    );
}

/// The `codex)` case body of the wrapper, sliced between the case
/// label and its `;;` terminator — so negative assertions (no
/// `--mcp-config`) can't be fouled by the claude-code branch's
/// legitimate use of the same flag.
fn wrapper_codex_branch() -> &'static str {
    let start = DEFAULT_WRAPPER
        .find("codex)")
        .expect("DEFAULT_WRAPPER has a codex) case");
    let body = &DEFAULT_WRAPPER[start..];
    let end = body.find(";;").expect("codex) case terminated by ;;");
    &body[..end]
}

/// Codex launch correctness: the real Codex CLI has no --mcp-config
/// or --instructions flags (TypeScript-era leftovers). MCP rides the
/// per-agent CODEX_HOME config.toml; instructions + reasoning effort
/// ride the repeatable `-c KEY=VALUE` override. Pin the shapes so a
/// silent wrapper edit can't reintroduce the phantom flags and break
/// every codex spawn at argv parsing.
#[test]
fn wrapper_codex_uses_config_overrides_not_phantom_flags() {
    let codex = wrapper_codex_branch();
    assert!(
        codex.contains("-c \"model_reasoning_effort=$EFFORT\""),
        "codex branch must pass effort via -c model_reasoning_effort",
    );
    assert!(
        codex.contains("-c \"model_instructions_file=$SYSTEM_PROMPT_PATH\""),
        "codex branch must pass the role prompt via -c model_instructions_file",
    );
    // Match the full flag usage, not the bare flag name — the codex
    // branch's comments legitimately name the nonexistent flags to
    // explain why the -c overrides are used instead.
    assert!(
        !codex.contains("--mcp-config \"$MCP_CONFIG\""),
        "codex has no --mcp-config flag — MCP goes through CODEX_HOME/config.toml",
    );
    assert!(
        !codex.contains("--instructions \"$SYSTEM_PROMPT_PATH\""),
        "codex has no --instructions flag",
    );
    assert!(
        codex.contains("export CODEX_HOME"),
        "codex branch must export the per-agent CODEX_HOME",
    );
}

/// Codex permission mapping mirrors the claude-code branch's
/// semantics: attended → codex's own interactive default (no flags),
/// bypassPermissions → --yolo, headless default → `-a never` plus the
/// workspace-write sandbox (approvals can't prompt in an unattended
/// pane; the sandbox boundary is the guardrail).
#[test]
fn wrapper_codex_permission_mapping_present() {
    let codex = wrapper_codex_branch();
    assert!(
        codex.contains("-a never -s workspace-write"),
        "codex headless default must be -a never -s workspace-write",
    );
    assert!(
        codex.contains("--yolo"),
        "codex bypassPermissions must map to --yolo",
    );
    assert!(
        codex.contains("[ \"${PERMISSION_MODE:-}\" = \"attended\" ]"),
        "codex branch must keep the set -u-safe attended opt-out",
    );
}

/// Codex's first-run "trust this folder" dialog defaults to the
/// trust option, so the auto-confirm watcher accepts it with one
/// Enter — otherwise the first spawn in a new directory strands an
/// unattended pane. The dialog wording has drifted across codex
/// releases, so pin BOTH observed variants (and the codex arm
/// opting into the watcher) so a silent edit can't re-strand codex
/// boots on either wording.
#[test]
fn wrapper_codex_trust_dialog_auto_confirmed() {
    assert!(
        DEFAULT_WRAPPER.contains("Yes, I trust this folder"),
        "auto-confirm watcher must match codex's classic trust-folder wording",
    );
    assert!(
        DEFAULT_WRAPPER.contains("Yes, allow Codex to work in this folder"),
        "auto-confirm watcher must match codex's newer trust-folder wording",
    );
    assert!(
        DEFAULT_WRAPPER.contains("Do you trust the contents of this directory"),
        "auto-confirm watcher must match codex 0.144's trust-dialog wording",
    );
    // Co-occurrence guard: the codex wordings must only fire alongside
    // dialog chrome, or an agent that merely PRINTS a wording in prose
    // (this team discusses these dialogs constantly) triggers a stray
    // Enter. "Yes, I trust this folder" doubles as claude's own trust
    // option label, which makes the single-string form doubly unsafe.
    assert!(
        DEFAULT_WRAPPER.contains("Press enter to continue|2\\. No"),
        "codex trust match must require dialog chrome co-occurrence",
    );
    assert!(
        wrapper_codex_branch().contains("AUTO_CONFIRM=1"),
        "codex branch must opt into the auto-confirm watcher",
    );
}

/// Codex session resume: the per-agent CODEX_HOME isolates the
/// session store, so the wrapper probes it for rollout JSONLs
/// (sessions/YYYY/MM/DD layout) and reopens the newest one with
/// `codex resume --last` — subcommand before flags. The resume path
/// must NOT re-inject the bootstrap positional (whether `resume`
/// accepts a PROMPT is unverified upstream); a one-shot tmux nudge
/// re-grounds the agent instead. Pin the probe glob, the subcommand
/// shape, the fresh-only positional, and the nudge so a silent edit
/// can't regress any half.
#[test]
fn wrapper_codex_resume_branch_present() {
    let codex = wrapper_codex_branch();
    assert!(
        codex.contains("set -- resume --last"),
        "codex resume path must lead the argv with the resume subcommand",
    );
    assert!(
        codex.contains("\"$CODEX_HOME/sessions\"/*/*/*/*.jsonl"),
        "codex resume probe must glob the sessions/YYYY/MM/DD rollout layout",
    );
    assert!(
        codex.contains("[ \"$RESUMED\" = 0 ] && set -- \"$@\" \"$BOOTSTRAP_PROMPT\""),
        "codex must pass the bootstrap positional on fresh spawns only",
    );
    assert!(
        DEFAULT_WRAPPER.contains("nudge_resumed_session"),
        "wrapper must define the resumed-session wake-up nudge",
    );
    assert!(
        DEFAULT_WRAPPER.contains("Restarted mid-shift: call inbox_peek"),
        "nudge must point the resumed agent at inbox_peek catch-up",
    );
    // The nudge must re-fire Enter after a beat: a resumed codex TUI
    // loads history before the composer goes live, so the first Enter
    // can be eaten mid-boot and the text strands unsubmitted (observed
    // live on codex 0.144.3).
    let nudge_start = DEFAULT_WRAPPER
        .find("nudge_resumed_session() {")
        .expect("nudge helper present");
    let nudge = &DEFAULT_WRAPPER[nudge_start..nudge_start + 700];
    assert!(
        nudge.matches("send-keys").count() >= 2,
        "nudge must send a trailing bare Enter to beat the resume-boot race",
    );
}

/// Resume crash-loop self-heal: a corrupt session store makes the
/// resume path (`codex resume --last`, `opencode -c`) die instantly
/// on every boot, and the resume probe re-matches it forever. The
/// wrapper counts consecutive fast resume-path exits (a single
/// counter shared by both runtimes — the probes set the shared
/// RESUMED flag) and moves the runtime's session store aside after
/// the third, so the next boot is fresh. Pin the counter, the
/// threshold, and both move-aside paths so a silent edit can't
/// reintroduce the infinite crash loop for either runtime.
#[test]
fn wrapper_resume_crash_loop_self_heal_present() {
    for marker in [
        "RESUME_FAST_FAILS=$((RESUME_FAST_FAILS + 1))",
        "[ \"$RESUME_FAST_FAILS\" -ge 3 ]",
        "$CODEX_HOME/sessions.crash-bak",
        "$OC_HOME/db.crash-bak",
        "mv \"$OPENCODE_DB\" \"$OPENCODE_DB-shm\" \"$OPENCODE_DB-wal\" \"$OC_HOME/db.crash-bak/\"",
    ] {
        assert!(
            DEFAULT_WRAPPER.contains(marker),
            "DEFAULT_WRAPPER missing marker: {marker}",
        );
    }
}

/// The opencode case body of the wrapper, sliced between the case
/// label and its `;;` terminator — same trap as
/// [`wrapper_codex_branch`]: negative assertions must not be fouled
/// by other branches' legitimate use of the same flags. (The
/// wrapper itself never writes the literal case-label string in
/// comments outside the branch, so `find` lands on the real label.)
fn wrapper_opencode_branch() -> &'static str {
    let start = DEFAULT_WRAPPER
        .find("opencode)")
        .expect("DEFAULT_WRAPPER has an opencode case");
    let body = &DEFAULT_WRAPPER[start..];
    let end = body.find(";;").expect("opencode case terminated by ;;");
    &body[..end]
}

/// OpenCode launch correctness: MCP + instructions ride the
/// per-agent OPENCODE_CONFIG json (no --mcp-config flag), the TUI
/// auto-upgrades the shared binary in place unless
/// OPENCODE_DISABLE_AUTOUPDATE=1 is exported (verified
/// 1.17.13→1.17.18 mid-run), missing auth silently falls back to
/// free anonymous models (so the wrapper must warn), and `effort:`
/// has no TUI mapping (`--variant` is run-subcommand only). Pin all
/// four so a silent wrapper edit can't regress any of them.
#[test]
fn wrapper_opencode_launch_shape_present() {
    let opencode = wrapper_opencode_branch();
    assert!(
        opencode.contains("export OPENCODE_DISABLE_AUTOUPDATE=1"),
        "opencode branch must disable the in-place binary autoupdate",
    );
    assert!(
        opencode.contains("$HOME/.local/share/opencode/auth.json")
            && opencode.contains("free anonymous models"),
        "opencode branch must warn when auth.json is absent (silent free-tier fallback)",
    );
    assert!(
        opencode.contains("--model \"$MODEL\""),
        "opencode branch must thread model (provider/model form)",
    );
    assert!(
        !opencode.contains("--mcp-config \"$MCP_CONFIG\""),
        "opencode has no --mcp-config flag — MCP goes through OPENCODE_CONFIG json",
    );
    assert!(
        !opencode.contains("--effort") && !opencode.contains("--variant \""),
        "effort is unsupported on opencode v1 — no flag may be wired",
    );
    assert!(
        !opencode.contains("AUTO_CONFIRM=1"),
        "opencode boots straight to the composer — no dialogs to auto-confirm",
    );
}

/// OpenCode permission mapping: attended keeps opencode's own
/// interactive ask-prompts; everything else — including
/// bypassPermissions, since opencode has no full-bypass equivalent
/// (--yolo closed not-planned upstream) — maps to `--auto`
/// (auto-approve anything not explicitly denied).
#[test]
fn wrapper_opencode_permission_mapping_present() {
    let opencode = wrapper_opencode_branch();
    assert!(
        opencode.contains("set -- \"$@\" --auto"),
        "opencode headless default must be --auto",
    );
    // Match the full flag usage, not the bare flag name — the
    // branch's comments legitimately name codex's --yolo to explain
    // why bypassPermissions downgrades to --auto here.
    assert!(
        !opencode.contains("set -- \"$@\" --yolo"),
        "opencode has no full-bypass flag — bypassPermissions downgrades to --auto",
    );
    assert!(
        opencode.contains("[ \"${PERMISSION_MODE:-}\" = \"attended\" ]"),
        "opencode branch must keep the set -u-safe attended opt-out",
    );
}

/// OpenCode session resume: the per-agent OPENCODE_DB isolates the
/// session store, so the wrapper probes for the db file and
/// continues the prior conversation with `-c` — scoped to the cwd
/// within that db, which is exact per agent. The resume path must
/// NOT pass `--prompt` (the one-shot nudge re-grounds the agent);
/// fresh spawns pass the bootstrap via `--prompt`, a flag, not a
/// positional. Pin the probe, the flag selection, and the set -u
/// defaults for the env-file-rendered vars.
#[test]
fn wrapper_opencode_resume_branch_present() {
    let opencode = wrapper_opencode_branch();
    assert!(
        opencode.contains("[ -n \"$OPENCODE_DB\" ] && [ -f \"$OPENCODE_DB\" ]"),
        "opencode resume probe must check the per-agent db file",
    );
    assert!(
        opencode.contains("set -- \"$@\" -c"),
        "opencode resume path must continue via -c",
    );
    assert!(
        opencode.contains("[ \"$RESUMED\" = 0 ] && set -- \"$@\" --prompt \"$BOOTSTRAP_PROMPT\""),
        "opencode must pass the bootstrap via --prompt on fresh spawns only",
    );
    for marker in [": \"${OPENCODE_DB:=}\"", ": \"${OPENCODE_CONFIG:=}\""] {
        assert!(
            DEFAULT_WRAPPER.contains(marker),
            "DEFAULT_WRAPPER missing set -u default: {marker}",
        );
    }
}

/// T-174: the wrapper picks `--resume` vs `--session-id` by
/// probing the on-disk session jsonl. A silent edit that drops
/// the resume branch would re-trigger "Session ID is already in
/// use" on the second `teamctl up` under claude 2.1.138+; an edit
/// that drops the create branch would break first-launch.
/// Pin both markers plus the glob shape so neither half regresses.
#[test]
fn wrapper_session_id_resume_branch_present() {
    for marker in [
        "--session-id \"$CLAUDE_SESSION_ID\"",
        "--resume \"$CLAUDE_SESSION_ID\"",
        "$HOME/.claude/projects/",
        "$CLAUDE_SESSION_ID.jsonl",
    ] {
        assert!(
            DEFAULT_WRAPPER.contains(marker),
            "DEFAULT_WRAPPER missing marker: {marker}",
        );
    }
}

/// T-190: the wrapper runs under `set -u`. Both
/// `CLAUDE_SESSION_ID` and `CLAUDE_SESSION_NAME` are rendered
/// into the env file only for `runtime: claude-code` agents
/// (team-core::render::render_env). If they're absent for any
/// reason — env file from an older render, write race, future
/// runtime variant — the unguarded `[ -n "$CLAUDE_SESSION_ID" ]`
/// reference aborts the wrapper, the tmux pane closes, and the
/// supervisor marks the agent stopped without a diagnostic.
/// The defaults at the top of the wrapper close that hole; a
/// silent edit that drops them re-opens the failure mode.
#[test]
fn wrapper_session_vars_have_set_u_defaults() {
    for marker in [
        ": \"${CLAUDE_SESSION_ID:=}\"",
        ": \"${CLAUDE_SESSION_NAME:=}\"",
    ] {
        assert!(
            DEFAULT_WRAPPER.contains(marker),
            "DEFAULT_WRAPPER missing marker: {marker}",
        );
    }
}

/// T-190: macOS ships bash 3.2 as `/bin/sh`. Bash 3.2 has a
/// parser bug where `${VAR:=DEFAULT}` cannot reliably parse
/// escape sequences inside DEFAULT (backslash-backtick,
/// backslash-quote). The wrapper's BOOTSTRAP_PROMPT default
/// contains both, so the pre-T-190 `${BOOTSTRAP_PROMPT:=...}`
/// shape aborted every spawn on macOS — the 0.8.0 fresh-install
/// regression. The fix is conditional assignment, not parameter
/// expansion: pin that BOTH the `:=` form is GONE and the
/// `[ -z ]` plain-assignment shape is present, so a future
/// cleanup can't silently regress macOS again.
#[test]
fn wrapper_bootstrap_prompt_default_is_macos_safe() {
    assert!(
        !DEFAULT_WRAPPER.contains("${BOOTSTRAP_PROMPT:="),
        "DEFAULT_WRAPPER still uses ${{VAR:=DEFAULT}} for \
             BOOTSTRAP_PROMPT — that shape is bash-3.2-fatal on \
             macOS when DEFAULT contains escape sequences. Keep \
             the conditional-assignment form.",
    );
    for marker in [
        "if [ -z \"${BOOTSTRAP_PROMPT:-}\" ]; then",
        "BOOTSTRAP_PROMPT=\"Begin your shift as ${AGENT}.",
    ] {
        assert!(
            DEFAULT_WRAPPER.contains(marker),
            "DEFAULT_WRAPPER missing marker: {marker}",
        );
    }
}

/// The bootstrap default block, sliced from the `[ -z ]` guard to its
/// closing `fi` — so per-runtime prompt assertions can't be fouled by
/// text elsewhere in the wrapper.
fn wrapper_bootstrap_block() -> &'static str {
    let start = DEFAULT_WRAPPER
        .find("if [ -z \"${BOOTSTRAP_PROMPT:-}\" ]; then")
        .expect("DEFAULT_WRAPPER has the bootstrap default guard");
    let body = &DEFAULT_WRAPPER[start..];
    let end = body.find("\nfi").expect("bootstrap guard closed by fi");
    &body[..end]
}

/// Delivery differs per runtime, so the bootstrap default must too:
/// claude-code keeps the Channels text (events pushed into the
/// session, no polling); every other runtime gets the honest story —
/// team-mcp types `📬 sender: "preview…" (+N more)` nudges into the
/// pane, and the agent starts with an `inbox_peek` catch-up. Telling
/// a non-claude agent "traffic is delivered as channel events"
/// strands it idle forever, because its runtime drops MCP
/// notifications.
#[test]
fn wrapper_bootstrap_prompt_dispatches_on_runtime() {
    let block = wrapper_bootstrap_block();
    assert!(
        block.contains("case \"$RUNTIME\" in"),
        "bootstrap default must dispatch on $RUNTIME",
    );
    let (claude, other) = block
        .split_once("*)")
        .expect("bootstrap case has a catch-all arm");
    assert!(
        claude.contains("Claude Code Channels") && claude.contains("you do not need to poll"),
        "claude-code prompt must keep the Channels delivery text",
    );
    assert!(
        other.contains("Call inbox_peek now"),
        "non-claude prompt must instruct the startup inbox_peek catch-up",
    );
    assert!(
        other.contains("(+N more)") && other.contains("inbox_read") && other.contains("inbox_ack"),
        "non-claude prompt must describe the nudge's preview shape and the drain loop",
    );
    assert!(
        !other.to_lowercase().contains("channel"),
        "non-claude prompt must not claim channel-event delivery",
    );
}

fn compose_with_multi_role_prompt(root: &Path, project_id: &str) -> Compose {
    let mut managers = BTreeMap::new();
    managers.insert(
        "mgr".into(),
        Agent {
            runtime: "claude-code".into(),
            model: None,
            role_prompt: Some(RolePrompt::Multiple(vec![
                PathBuf::from("roles/_base.md"),
                PathBuf::from("roles/mgr.md"),
            ])),
            permission_mode: None,
            autonomy: "low_risk_only".into(),
            can_dm: vec![],
            can_broadcast: vec![],
            reports_to: None,
            on_rate_limit: None,
            effort: None,
            ultracode: false,
            interfaces: None,
            display_name: None,
            hooks: vec![],
            mcps: Default::default(),
            subagents: vec![],
            skills: vec![],
        },
    );
    Compose {
        root: root.to_path_buf(),
        global: Global {
            version: team_core::compose::SchemaVersion::new("2.0.0"),
            broker: Default::default(),
            supervisor: Default::default(),
            budget: Default::default(),
            hitl: Default::default(),
            rate_limits: Default::default(),
            interfaces: vec![],
            projects: vec![],
            attachments: Default::default(),
        },
        projects: vec![Project {
            version: 2,
            project: ProjectMeta {
                id: project_id.into(),
                name: project_id.into(),
                cwd: root.to_path_buf(),
            },
            channels: vec![],
            managers,
            workers: Default::default(),
            interfaces: None,
        }],
    }
}

#[test]
fn render_project_public_writes_role_prompt_concat() {
    // Regression for T-103 qa finding: the scoped reload path
    // must materialize the multi-file role_prompt concat too,
    // else editing a source file and running
    // `teamctl reload <project>` boots the agent against a stale
    // concat file (zombie-prompt).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("roles")).unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::write(root.join("roles/_base.md"), "BASE").unwrap();
    std::fs::write(root.join("roles/mgr.md"), "MGR").unwrap();

    let compose = compose_with_multi_role_prompt(root, "p");
    render_project_public(&compose, "p").expect("render_project_public");

    // #428: render must create the heartbeat dir so the rendered bare
    // `touch` hook has somewhere to write. If this regresses, the dir is
    // missing => `touch` fails => no marker => every agent reads Idle
    // (the safe failure, but a silent loss of the working signal).
    assert!(
        root.join("state/heartbeats").is_dir(),
        "render must create state/heartbeats/"
    );

    let concat = role_prompt_concat_path(root, "p", "mgr");
    let got = std::fs::read_to_string(&concat).expect("concat file written");
    assert_eq!(got, "BASE\n\n—\n\nMGR");

    // No zombies: a source edit + re-render must update the concat.
    std::fs::write(root.join("roles/_base.md"), "BASE-v2").unwrap();
    render_project_public(&compose, "p").expect("render_project_public re-run");
    let got = std::fs::read_to_string(&concat).unwrap();
    assert_eq!(got, "BASE-v2\n\n—\n\nMGR");
}

/// Serializes the HOME-mutating test(s) in this binary; `$HOME` is
/// process-global, so a concurrent reader/writer would race.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Point `$HOME` at `home` for the guard's lifetime, then restore it, so
/// the trust write lands in a throwaway `.claude.json`, never the real one.
struct HomeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn set(home: &Path) -> Self {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("HOME");
        // SAFETY: HOME_LOCK serializes every HOME mutation in this binary,
        // matching the `unsafe { set_var }` convention elsewhere in up.rs.
        unsafe { std::env::set_var("HOME", home) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: still holding HOME_LOCK (see `set`).
        match &self.prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

/// `ensure_claude_trust` pre-accepts Claude's workspace-trust dialog by
/// writing `hasTrustDialogAccepted: true` under each claude-code agent's
/// cwd, and is a no-op on a second run (trust already on disk) so the `up`
/// notice doesn't nag on every restart.
#[test]
fn ensure_claude_trust_writes_key_then_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".team");
    std::fs::create_dir_all(root.join("projects")).unwrap();
    std::fs::write(
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
    std::fs::write(
        root.join("projects/hello.yaml"),
        r#"
version: 2
project:
  id: hello
  name: Hello
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
"#,
    )
    .unwrap();
    let compose = Compose::load(&root).expect("compose loads");

    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let config_path = home.join(".claude.json");
    let _guard = HomeGuard::set(&home);

    // First run writes the trust key for the agent's (canonicalized) cwd.
    ensure_claude_trust(&compose).expect("first ensure_claude_trust");
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("wrote .claude.json"))
            .expect("config is valid json");
    let key = root.canonicalize().unwrap().display().to_string();
    assert_eq!(
        cfg["projects"][&key]["hasTrustDialogAccepted"],
        serde_json::Value::Bool(true),
        "trust key must be written for the agent cwd; config: {cfg}",
    );

    // Second run is a no-op: trust is on disk, so nothing is rewritten
    // (and the operator notice does not re-fire).
    let before = std::fs::read_to_string(&config_path).unwrap();
    ensure_claude_trust(&compose).expect("second ensure_claude_trust");
    let after = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(before, after, "second run must not rewrite the config");
}

/// #430: the boot-context asset must carry the REQUIRED `hookEventName`
/// inside `hookSpecificOutput` — without it Claude Code silently drops
/// `additionalContext` and the hook injects nothing while still exiting 0
/// (the exact silent-no-op the de-risk pass caught). Pin that, plus the
/// `SessionStart` event name and the wake-aware verb mapping, so a future
/// edit can't quietly hollow the asset out.
#[test]
fn boot_script_emits_session_start_context() {
    assert!(
        DEFAULT_BOOT_SCRIPT.contains(r#""hookEventName":"SessionStart""#),
        "boot.sh must emit the required hookEventName or CC drops additionalContext"
    );
    assert!(
        DEFAULT_BOOT_SCRIPT.contains("additionalContext"),
        "boot.sh must emit additionalContext"
    );
    for verb in ["resumed", "cleared context", "compacted", "booted"] {
        assert!(
            DEFAULT_BOOT_SCRIPT.contains(verb),
            "boot.sh missing wake-aware verb: {verb}"
        );
    }
    // POSIX `/bin/sh` shebang + `set -u`, matching the wrapper's contract
    // (the command runs in the agent's shell, macOS bash 3.2 included).
    assert!(DEFAULT_BOOT_SCRIPT.starts_with("#!/bin/sh"));
    assert!(DEFAULT_BOOT_SCRIPT.contains("set -u"));
    // #439: the two source-specific extensions and the argv-optional
    // guards. The `${1:-}` / `${2:-}` reads keep the script `set -u`-safe
    // when an older rendered hook passes no argv (downtime then omits).
    assert!(
        DEFAULT_BOOT_SCRIPT.contains("${1:-}") && DEFAULT_BOOT_SCRIPT.contains("${2:-}"),
        "boot.sh must guard its optional argv under set -u"
    );
    assert!(
        DEFAULT_BOOT_SCRIPT.contains("You were down for"),
        "boot.sh must carry the downtime sentence"
    );
    assert!(
        DEFAULT_BOOT_SCRIPT.contains("Re-anchor before continuing"),
        "boot.sh must carry the compact re-anchor copy"
    );
}

/// #439: execute the real boot.sh asset on the host shell and assert the
/// wake notice per `source` × file-state. Each run exercises only its
/// host's native branch — macOS takes the BSD `stat -f`/`date -r` path,
/// Linux the GNU `stat -c`/`date -d` path — so it is the CI matrix across
/// BOTH the macos-14 and ubuntu-24.04 legs that proves the two portability
/// fallbacks on real hardware, not just reasoned about. Mtimes are set
/// precisely via `File::set_modified` (in std since 1.75, under our 1.86
/// MSRV) so the buckets are deterministic.
#[test]
fn boot_script_reports_downtime_and_reanchor() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, SystemTime};

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("boot.sh");
    std::fs::write(&script, DEFAULT_BOOT_SCRIPT).unwrap();

    // Run boot.sh under /bin/sh with the given stdin source + argv paths,
    // returning the parsed `additionalContext`. Parsing as JSON also
    // enforces the ASCII no-escaping contract: a stray quote in any
    // injected copy would break this parse and fail the test.
    let run = |source: &str, args: &[&std::path::Path]| -> String {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(&script);
        for a in args {
            cmd.arg(a);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let payload = format!("{{\"source\":\"{source}\"}}");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "boot.sh exited non-zero for source={source}"
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // A file whose mtime is `secs_ago` seconds before now.
    let aged = |name: &str, secs_ago: u64| -> std::path::PathBuf {
        let p = dir.path().join(name);
        let f = std::fs::File::create(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
            .unwrap();
        p
    };
    let missing = dir.path().join("does-not-exist");

    // source variants, no argv: base notice only, with the compact
    // re-anchor appended on `compact`.
    let startup = run("startup", &[]);
    assert!(startup.starts_with("You booted at "), "{startup}");
    assert!(
        !startup.contains("You were down"),
        "no argv => no downtime: {startup}"
    );
    for (src, lead) in [
        ("resume", "You resumed at "),
        ("clear", "You cleared context at "),
    ] {
        let out = run(src, &[]);
        assert!(out.starts_with(lead), "{out}");
        assert!(
            !out.contains("You were down") && !out.contains("Re-anchor"),
            "{src} must stay the base notice: {out}"
        );
    }
    let compact = run("compact", &[]);
    assert!(compact.starts_with("You compacted at "), "{compact}");
    assert!(
        compact.contains("Re-anchor before continuing: re-read your working files"),
        "compact must carry the re-anchor copy: {compact}"
    );

    // startup downtime from LASTSEEN ($1), marker ($2) absent.
    assert!(
        run("startup", &[&aged("ls_2h", 7200), &missing])
            .contains("You were down for about 2 hours (last active "),
        "2h lastseen => 2 hours"
    );
    assert!(
        run("startup", &[&aged("ls_30s", 30), &missing])
            .contains("You were down for under a minute (last active "),
        "30s lastseen => under a minute"
    );

    // Unclean shutdown: the marker survived and is fresher than LASTSEEN,
    // so its mtime wins.
    assert!(
        run("startup", &[&aged("l_2h", 7200), &aged("m_10m", 600)])
            .contains("You were down for about 10 minutes "),
        "present marker (10m) beats lastseen (2h)"
    );

    // Omit cases: both files missing, and a non-startup source never
    // reports downtime even with a usable file present.
    assert!(
        !run("startup", &[&missing, &missing]).contains("You were down"),
        "both missing => omit"
    );
    assert!(
        !run("resume", &[&aged("ls_for_resume", 7200), &missing]).contains("You were down"),
        "resume must not report downtime"
    );
}

/// #430: `teamctl up` materializes `bin/boot.sh` next to the wrapper, with
/// the embedded content and a 0o755 mode, so Claude Code's SessionStart
/// hook can execute it. Mirrors the wrapper's managed-asset contract.
#[test]
fn ensure_wrapper_and_dirs_writes_executable_boot_script() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".team");
    std::fs::create_dir_all(root.join("projects")).unwrap();
    std::fs::write(
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
    std::fs::write(
        root.join("projects/hello.yaml"),
        r#"
version: 2
project:
  id: hello
  name: Hello
  cwd: .
managers:
  manager:
    runtime: claude-code
    model: claude-opus-4-8
"#,
    )
    .unwrap();
    let compose = Compose::load(&root).expect("compose loads");

    ensure_wrapper_and_dirs(&compose).expect("ensure_wrapper_and_dirs");

    let boot = team_core::render::boot_script_path(&compose.root);
    assert!(boot.is_file(), "bin/boot.sh must be written");
    assert_eq!(
        std::fs::read_to_string(&boot).unwrap(),
        DEFAULT_BOOT_SCRIPT,
        "on-disk boot.sh must match the embedded asset"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&boot).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "boot.sh must be chmod 0o755");
    }
}

/// Minimal spec for exercising [`freshen_for_spec`] — only the
/// project/agent pair matters (they key the codex-home path).
fn spec_for(root: &Path, project: &str, agent: &str) -> AgentSpec {
    AgentSpec {
        project: project.into(),
        agent: agent.into(),
        tmux_session: format!("test-{project}-{agent}"),
        wrapper: root.join("wrapper.sh"),
        cwd: root.to_path_buf(),
        env_file: root.join("env"),
    }
}

/// T-352 parity: the codex `--fresh` arm moves the sessions dir
/// aside as a `sessions.bak` recovery copy (matching Claude's
/// `.bak`), leaves config.toml and the auth.json symlink at the
/// codex-home root untouched, treats a missing sessions dir as a
/// silent no-op (keeping any earlier backup), and replaces a
/// stale backup from an earlier `--fresh`.
#[test]
fn freshen_codex_moves_sessions_aside_keeping_home_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let spec = spec_for(root, "hello", "dev");
    let home = codex_home_dir(root, "hello", "dev");
    let sessions = home.join("sessions");
    let bak = home.join("sessions.bak");

    // Stage a rollout plus the root-level files the wrapper relies on.
    std::fs::create_dir_all(sessions.join("2026/07/01")).unwrap();
    std::fs::write(sessions.join("2026/07/01/rollout-1.jsonl"), "v1").unwrap();
    std::fs::write(home.join("config.toml"), "[mcp_servers.team]").unwrap();
    let real_auth = root.join("real-auth.json");
    std::fs::write(&real_auth, "{}").unwrap();
    std::os::unix::fs::symlink(&real_auth, home.join("auth.json")).unwrap();

    freshen_for_spec(root, &spec, "codex", true);
    assert!(!sessions.exists(), "sessions dir must be moved aside");
    assert_eq!(
        std::fs::read_to_string(bak.join("2026/07/01/rollout-1.jsonl")).unwrap(),
        "v1",
        "recovery copy must carry the rollout",
    );
    assert!(home.join("config.toml").is_file(), "config.toml survives");
    assert!(
        home.join("auth.json")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "auth.json symlink survives as a symlink",
    );

    // Missing sessions dir: silent no-op that keeps the backup.
    freshen_for_spec(root, &spec, "codex", true);
    assert!(
        bak.join("2026/07/01/rollout-1.jsonl").is_file(),
        "no-op freshen must not clobber the existing backup",
    );

    // A newer session replaces the pre-existing .bak wholesale.
    std::fs::create_dir_all(sessions.join("2026/07/02")).unwrap();
    std::fs::write(sessions.join("2026/07/02/rollout-2.jsonl"), "v2").unwrap();
    freshen_for_spec(root, &spec, "codex", true);
    assert!(!sessions.exists());
    assert!(
        !bak.join("2026/07/01/rollout-1.jsonl").exists(),
        "stale backup must be replaced, not merged into",
    );
    assert_eq!(
        std::fs::read_to_string(bak.join("2026/07/02/rollout-2.jsonl")).unwrap(),
        "v2",
    );
}

/// OpenCode `--fresh` parity: the session db moves into a `db.bak/`
/// recovery copy together with its -shm/-wal sidecars (absent
/// sidecars skipped), opencode.json at the home root is untouched,
/// a missing db is a silent no-op (keeping any earlier backup),
/// and a later `--fresh` replaces a stale backup wholesale.
#[test]
fn freshen_opencode_moves_db_aside_keeping_home_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let spec = spec_for(root, "hello", "dev");
    let home = team_core::render::opencode_home_dir(root, "hello", "dev");
    let bak = home.join("db.bak");

    // Stage the db + one sidecar plus the managed config the
    // wrapper relies on (the -shm sidecar is deliberately absent —
    // WAL sidecars only exist while sqlite holds the db open).
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("agent.db"), "v1").unwrap();
    std::fs::write(home.join("agent.db-wal"), "wal1").unwrap();
    std::fs::write(home.join("opencode.json"), "{}").unwrap();

    freshen_for_spec(root, &spec, "opencode", true);
    assert!(!home.join("agent.db").exists(), "db must be moved aside");
    assert!(!home.join("agent.db-wal").exists(), "sidecar moves too");
    assert_eq!(std::fs::read_to_string(bak.join("agent.db")).unwrap(), "v1");
    assert_eq!(
        std::fs::read_to_string(bak.join("agent.db-wal")).unwrap(),
        "wal1",
    );
    assert!(
        home.join("opencode.json").is_file(),
        "managed config survives"
    );

    // Missing db: silent no-op that keeps the backup.
    freshen_for_spec(root, &spec, "opencode", true);
    assert_eq!(
        std::fs::read_to_string(bak.join("agent.db")).unwrap(),
        "v1",
        "no-op freshen must not clobber the existing backup",
    );

    // A newer session replaces the pre-existing db.bak wholesale.
    std::fs::write(home.join("agent.db"), "v2").unwrap();
    freshen_for_spec(root, &spec, "opencode", true);
    assert!(!home.join("agent.db").exists());
    assert_eq!(std::fs::read_to_string(bak.join("agent.db")).unwrap(), "v2");
    assert!(
        !bak.join("agent.db-wal").exists(),
        "stale backup must be replaced, not merged into",
    );
}

#[test]
fn invalid_claude_config_is_preserved_and_fails_visibly() {
    let dir = tempfile::tempdir().unwrap();
    let compose = compose_with_multi_role_prompt(dir.path(), "p");
    let home = dir.path().join("home");
    fs::create_dir(&home).unwrap();
    let config = home.join(".claude.json");
    let _guard = HomeGuard::set(&home);
    for original in ["{invalid", "[]", "null"] {
        fs::write(&config, original).unwrap();
        assert!(ensure_claude_trust(&compose).is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
    }
}
