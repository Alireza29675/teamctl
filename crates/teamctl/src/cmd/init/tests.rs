use super::*;

#[test]
fn paint_with_emits_ansi_only_when_color_on() {
    // T-181: the init team-structure preview must honor NO_COLOR /
    // non-TTY via `use_color`. `paint_with` is the gate's testable core.
    let colored = paint_with("38;5;218", "ada", true);
    assert_eq!(colored, "\x1b[38;5;218mada\x1b[0m");

    let plain = paint_with("38;5;218", "ada", false);
    assert_eq!(plain, "ada");
    assert!(
        !plain.contains('\x1b'),
        "NO_COLOR / non-TTY output must carry no ANSI, got: {plain:?}"
    );
}

#[test]
fn confirm_defaults_to_yes_on_empty_declines_on_n() {
    // #356: the guided intent prompt defaults to Yes — a bare Enter
    // (and y/yes, any case) proceeds; only an explicit n/no declines.
    assert!(answer_is_yes(""));
    assert!(answer_is_yes("\n"));
    assert!(answer_is_yes("  "));
    assert!(answer_is_yes("y"));
    assert!(answer_is_yes("Y\n"));
    assert!(answer_is_yes("yes"));
    assert!(!answer_is_yes("n"));
    assert!(!answer_is_yes("no"));
    assert!(!answer_is_yes("N"));
}

#[test]
fn slugify_basic() {
    assert_eq!(slugify("My Repo!"), "my-repo");
    assert_eq!(slugify("---weird___name"), "weird---name");
}

#[test]
fn substitute_replaces_handlebars() {
    let mut m = BTreeMap::new();
    m.insert("x", "Y".to_string());
    assert_eq!(substitute("hi {{x}}", &m), "hi Y");
}

#[test]
fn picker_empty_input_defaults_to_ideate_and_build() {
    // Menu order is Ideate & Build, Essentials, Guided, Blank. A bare
    // Enter lands the operator on slot 1 — the flagship — which is the
    // path we most want to be the default.
    match parse_picker_input("") {
        Choice::Template(t) => assert_eq!(t.key, "ideate-and-build"),
        _ => panic!("expected Choice::Template(ideate-and-build) for empty input"),
    }
}

#[test]
fn picker_one_selects_ideate_and_build() {
    match parse_picker_input("1") {
        Choice::Template(t) => assert_eq!(t.key, "ideate-and-build"),
        _ => panic!("expected Choice::Template(ideate-and-build) for input `1`"),
    }
}

#[test]
fn picker_two_selects_guided() {
    // Essentials is hidden from the picker, so Guided sits at slot 2
    // (handled out-of-band by the caller).
    assert!(matches!(parse_picker_input("2"), Choice::Guided));
}

#[test]
fn picker_three_selects_blank() {
    match parse_picker_input("3") {
        Choice::Template(t) => assert_eq!(t.key, "blank"),
        _ => panic!("expected Choice::Template(blank) for input `3`"),
    }
}

#[test]
fn picker_essentials_hidden_but_flag_still_resolves() {
    // Essentials is not on the interactive menu, but `--template
    // essentials` must still resolve to a real template.
    assert!(
        TEMPLATES.iter().any(|t| t.key == "essentials"),
        "essentials must remain in TEMPLATES for the --template flag"
    );
    assert!(
        !picker_menu()
            .iter()
            .any(|c| matches!(c, Choice::Template(t) if t.key == "essentials")),
        "essentials must NOT appear in the interactive picker menu"
    );
}

#[test]
fn picker_out_of_range_falls_back_to_ideate_and_build() {
    // Out-of-range / unparseable lands on slot 1 (the default), same
    // as a bare Enter.
    for input in ["99", "0", "hello"] {
        match parse_picker_input(input) {
            Choice::Template(t) => assert_eq!(
                t.key, "ideate-and-build",
                "input `{input}` should fall back to the flagship"
            ),
            _ => panic!("expected Choice::Template(ideate-and-build) for input `{input}`"),
        }
    }
}

#[test]
fn template_keys_lists_guided_first() {
    // Error-message ordering matters: Guided leads the known-keys list
    // the operator reads when they mistype `--template`.
    let keys = template_keys();
    assert!(
        keys.starts_with("guided"),
        "expected `guided` to lead; got `{keys}`"
    );
    assert!(keys.contains("ideate-and-build"));
    assert!(keys.contains("essentials"));
    assert!(keys.contains("blank"));
}

// ── `entries()` refactor (#395) ───────────────────────────────────
// Templates no longer hand-list files via `include_str!`; each carries
// an embedded `dir` walked recursively plus `shared` `_common/`
// overlays. These tests pin that mechanism so a future regression
// (a dropped file, a wrong dest path, a duplicated dotfile) is caught
// at the unit level — cli.rs already covers the materialized tree.

/// Find the contents for the entry at `dest`, if any. `entries()`
/// dest paths use `/` separators (include_dir yields root-relative
/// forward-slash paths), matching the literals the tests assert.
fn entry_for<'a>(entries: &'a [(String, &'static str)], dest: &str) -> Option<&'a str> {
    entries.iter().find(|(p, _)| p == dest).map(|(_, c)| *c)
}

#[test]
fn entries_ships_compose_and_gitignore_for_every_template() {
    // Core "no file silently dropped" guard: whatever the walk + shared
    // overlay produce, every file-shipping template must still yield a
    // compose root and a .gitignore, both with real contents.
    for tpl in TEMPLATES {
        let entries = tpl.entries();
        let compose = entry_for(&entries, "team-compose.yaml")
            .unwrap_or_else(|| panic!("template `{}` must ship team-compose.yaml", tpl.key));
        assert!(
            !compose.is_empty(),
            "template `{}` ships an empty team-compose.yaml",
            tpl.key
        );
        let gitignore = entry_for(&entries, ".gitignore")
            .unwrap_or_else(|| panic!("template `{}` must ship .gitignore", tpl.key));
        assert!(
            !gitignore.is_empty(),
            "template `{}` ships an empty .gitignore",
            tpl.key
        );
    }
}

#[test]
fn entries_walks_nested_dirs() {
    // The recursive walk must yield correct root-relative dest paths
    // for files nested under subdirectories, not just top-level files.
    let tpl = TEMPLATES
        .iter()
        .find(|t| t.key == "ideate-and-build")
        .expect("ideate-and-build template");
    let entries = tpl.entries();
    for dest in [
        "projects/main.yaml",
        "roles/_base.md",
        "subagents/implementer.md",
    ] {
        assert!(
            entry_for(&entries, dest).is_some(),
            "ideate-and-build entries() must include nested `{dest}`; got {:?}",
            entries.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
    }
}

#[test]
fn entries_includes_shared_common_overlays() {
    // `_common/` overlays live in a single source folder but land in
    // each template under the dest path the template's `shared` list
    // declares. Prove both blank dotfiles come through with the exact
    // single-source contents.
    let blank = TEMPLATES
        .iter()
        .find(|t| t.key == "blank")
        .expect("blank template");
    let blank_entries = blank.entries();
    assert_eq!(
        entry_for(&blank_entries, ".env.example"),
        Some(COMMON_ENV),
        "blank .env.example must be the shared _common/.env.example"
    );
    assert_eq!(
        entry_for(&blank_entries, ".gitignore"),
        Some(COMMON_GITIGNORE),
        "blank .gitignore must be the shared _common/.gitignore"
    );

    let essentials = TEMPLATES
        .iter()
        .find(|t| t.key == "essentials")
        .expect("essentials template");
    assert_eq!(
        entry_for(&essentials.entries(), ".gitignore"),
        Some(COMMON_GITIGNORE),
        "essentials .gitignore must be the shared _common/.gitignore"
    );
}

#[test]
fn entries_blank_does_not_duplicate_dotfiles() {
    // Guards a future change that both walks a `_common` file from the
    // template folder AND appends it via `shared` — each dotfile must
    // appear exactly once so the write loop doesn't clobber itself.
    let blank = TEMPLATES
        .iter()
        .find(|t| t.key == "blank")
        .expect("blank template");
    let entries = blank.entries();
    let count = |dest: &str| entries.iter().filter(|(p, _)| p == dest).count();
    assert_eq!(
        count(".gitignore"),
        1,
        "blank must ship exactly one .gitignore"
    );
    assert_eq!(
        count(".env.example"),
        1,
        "blank must ship exactly one .env.example"
    );
}

#[test]
fn entries_skips_ds_store() {
    // A stray macOS `.DS_Store` embedded at compile time must never
    // reach a scaffolded tree — the walk filters it by suffix.
    for tpl in TEMPLATES {
        for (path, _) in tpl.entries() {
            assert!(
                !path.ends_with(".DS_Store"),
                "template `{}` leaked a .DS_Store entry: `{path}`",
                tpl.key
            );
        }
    }
}

#[test]
fn every_template_file_is_utf8() {
    // `collect_dir` panics on a non-UTF-8 file (templates are text).
    // This makes a binary asset accidentally committed to a template
    // folder fail in CI rather than at runtime for an operator running
    // `teamctl init` — important since adding a file is meant to need
    // no code edit, so tests are the only guard.
    fn check(dir: &Dir<'static>, bad: &mut Vec<String>) {
        for f in dir.files() {
            if f.contents_utf8().is_none() {
                bad.push(f.path().to_string_lossy().into_owned());
            }
        }
        for d in dir.dirs() {
            check(d, bad);
        }
    }
    for tpl in TEMPLATES {
        let mut bad = Vec::new();
        check(tpl.dir, &mut bad);
        assert!(
            bad.is_empty(),
            "template `{}` has non-UTF-8 file(s): {bad:?}",
            tpl.key
        );
    }
}

#[test]
fn entries_lead_with_team_compose() {
    // The non-`--yes` preview leads with the file that defines the
    // team. Pin it so the readable ordering is a decision, not an
    // accident of the sort.
    for tpl in TEMPLATES {
        let entries = tpl.entries();
        assert_eq!(
            entries[0].0, "team-compose.yaml",
            "template `{}` should preview team-compose.yaml first, got `{}`",
            tpl.key, entries[0].0
        );
    }
}

#[test]
fn picker_menu_is_in_display_order() {
    // The displayed menu order is load-bearing (it maps typed numbers
    // to choices). Lock it: Ideate & Build, Guided, Blank. (Essentials
    // is intentionally hidden — see `picker_menu`.)
    let menu = picker_menu();
    assert_eq!(menu.len(), 3);
    assert!(matches!(menu[0], Choice::Template(t) if t.key == "ideate-and-build"));
    assert!(matches!(menu[1], Choice::Guided));
    assert!(matches!(menu[2], Choice::Template(t) if t.key == "blank"));
}

// ── init UX team-tree preview ─────────────────────────────────────
// The non-`--yes` preview gained a "Team shape:" reporting tree, a
// first-attach hint, and a `tree`-style file list (TreeNode). These
// pin that new logic: the hierarchy grouping, the orphan/degrade
// edges, the descriptor/label formatters, the attach-target pick, and
// the directory-tree renderer.

/// Strip ANSI escape sequences so tree assertions survive `cargo test`
/// run from an interactive terminal, where `team_structure_lines`
/// colours its output (stderr is a tty). A `\x1b…m` run is dropped.
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for d in chars.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Build the `files` slice `team_structure_lines` expects from a
/// compose body and a list of `(project relpath, project body)`.
fn tree_files(
    compose: &'static str,
    projects: &[(&str, &'static str)],
) -> Vec<(String, &'static str)> {
    let mut files: Vec<(String, &'static str)> = vec![("team-compose.yaml".to_string(), compose)];
    files.extend(projects.iter().map(|(p, c)| ((*p).to_string(), *c)));
    files
}

/// `team_structure_lines` with ANSI stripped per line, for assertions.
fn plain_tree(files: &[(String, &'static str)]) -> Vec<String> {
    team_structure_lines(files)
        .iter()
        .map(|l| strip_ansi(l))
        .collect()
}

/// Parse a `Project` body the same way `team_structure_lines` does, so
/// the agent-bearing tests can also poke `lead_attach_target` / the
/// descriptor formatters off the same fixtures.
fn parse_project(yaml: &str) -> team_core::compose::Project {
    serde_yaml::from_str::<team_core::compose::Project>(yaml)
        .unwrap_or_else(|e| panic!("fixture project must parse: {e}"))
}

const COMPOSE_ONE_DEMO: &str = "version: \"2.0.0\"\nprojects:\n  - file: projects/demo.yaml\n";

#[test]
fn tree_manager_with_two_workers_nests_and_id_sorts() {
    // A manager renders at the top of its project; the two workers that
    // `reports_to` it nest one indent deeper, in id-sorted order.
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss: {}
workers:
  zeta:
    reports_to: boss
  alpha:
    reports_to: boss
";
    let files = tree_files(COMPOSE_ONE_DEMO, &[("projects/demo.yaml", project)]);
    let lines = plain_tree(&files);

    assert_eq!(
        lines.iter().filter(|l| *l == "You").count(),
        1,
        "one agent-bearing project → exactly one `You`; got: {lines:?}"
    );
    // boss is the only (last) top-level entry → `└── `, flush-left.
    let boss = lines
        .iter()
        .find(|l| l.contains("boss"))
        .expect("boss line present");
    assert!(
        boss.starts_with("└── boss"),
        "manager sits at top level with a connector; got: {boss:?}"
    );
    // Workers nest under boss — deeper indent — and stay id-sorted.
    let alpha_i = lines
        .iter()
        .position(|l| l.contains("alpha"))
        .expect("alpha present");
    let zeta_i = lines
        .iter()
        .position(|l| l.contains("zeta"))
        .expect("zeta present");
    assert!(
        alpha_i < zeta_i,
        "workers are id-sorted (alpha before zeta); got: {lines:?}"
    );
    assert!(
        lines[alpha_i].starts_with("    "),
        "worker is indented under its manager; got: {:?}",
        lines[alpha_i]
    );
    assert!(
        lines[alpha_i].contains("├── alpha"),
        "first worker uses a tee connector; got: {:?}",
        lines[alpha_i]
    );
    assert!(
        lines[zeta_i].contains("└── zeta"),
        "last worker uses an elbow connector; got: {:?}",
        lines[zeta_i]
    );
}

#[test]
fn tree_two_managers_split_workers_by_reports_to() {
    // Each worker must nest under the manager it actually reports to,
    // not the first-declared one.
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  ay_mgr: {}
  zee_mgr: {}
workers:
  hand_a:
    reports_to: ay_mgr
  hand_z:
    reports_to: zee_mgr
";
    let files = tree_files(COMPOSE_ONE_DEMO, &[("projects/demo.yaml", project)]);
    let lines = plain_tree(&files);

    let pos = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("`{needle}` present; got: {lines:?}"))
    };
    // ay_mgr sorts first → tee; its worker sits right below it, before
    // zee_mgr appears.
    assert!(
        pos("ay_mgr") < pos("hand_a") && pos("hand_a") < pos("zee_mgr"),
        "hand_a nests under ay_mgr, above zee_mgr; got: {lines:?}"
    );
    assert!(
        pos("zee_mgr") < pos("hand_z"),
        "hand_z nests under zee_mgr; got: {lines:?}"
    );
    // ay_mgr is a non-last top-level manager, so its children render
    // under a `│   ` continuation column (the pipe shows zee_mgr still
    // follows). hand_a is ay_mgr's only/last child → elbow connector.
    assert!(
        lines[pos("hand_a")].starts_with("│   └── hand_a"),
        "hand_a is the only/last child of the non-last manager ay_mgr; got: {:?}",
        lines[pos("hand_a")]
    );
    // hand_z hangs off zee_mgr, the LAST top-level entry, so its
    // continuation column is blank (no trailing pipe).
    assert!(
        lines[pos("hand_z")].starts_with("    └── hand_z"),
        "hand_z is the last child of the last manager zee_mgr; got: {:?}",
        lines[pos("hand_z")]
    );
}

#[test]
fn tree_orphan_worker_hangs_at_top_level() {
    // A worker whose `reports_to` names no manager in this project is
    // an orphan: it renders at the top level beside the managers, not
    // nested under anyone.
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss: {}
workers:
  loose:
    reports_to: nobody
";
    let files = tree_files(COMPOSE_ONE_DEMO, &[("projects/demo.yaml", project)]);
    let lines = plain_tree(&files);

    let loose = lines
        .iter()
        .find(|l| l.contains("loose"))
        .expect("loose present");
    // Top-level → no leading-space indent before the connector.
    assert!(
        loose.starts_with("├── loose") || loose.starts_with("└── loose"),
        "orphan worker hangs at top level (flush-left connector); got: {loose:?}"
    );
}

#[test]
fn tree_agentless_project_yields_empty() {
    // A project with empty managers AND workers contributes nothing —
    // not even a bare `You`. (This is why `blank` shows no tree.)
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers: {}
workers: {}
";
    let files = tree_files(COMPOSE_ONE_DEMO, &[("projects/demo.yaml", project)]);
    assert!(
        team_structure_lines(&files).is_empty(),
        "agentless project must yield no preview lines"
    );
}

#[test]
fn tree_unparseable_compose_and_project_degrade_to_empty() {
    // Garbage compose → the fallback scans `projects/*.yaml`; a garbage
    // project there parses to nothing. Either way: empty Vec, no panic.
    let garbage_compose = tree_files(
        "this: is: not: valid: yaml: {[",
        &[("projects/demo.yaml", "also not valid: {[")],
    );
    assert!(
        team_structure_lines(&garbage_compose).is_empty(),
        "unparseable compose + project degrade to empty, never panic"
    );

    // A valid compose pointing at an unparseable project also degrades.
    let bad_project = tree_files(
        COMPOSE_ONE_DEMO,
        &[("projects/demo.yaml", "@@@ not yaml @@@")],
    );
    assert!(
        team_structure_lines(&bad_project).is_empty(),
        "valid compose + unparseable project degrades to empty"
    );
}

#[test]
fn tree_falls_back_to_scanning_projects_without_compose() {
    // No `team-compose.yaml` entry at all: the preview still works by
    // scanning any `projects/*.yaml` the template ships.
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss: {}
workers: {}
";
    let files: Vec<(String, &'static str)> = vec![("projects/demo.yaml".to_string(), project)];
    let lines = plain_tree(&files);
    assert_eq!(
        lines.iter().filter(|l| *l == "You").count(),
        1,
        "fallback scan previews the team without a compose; got: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("boss")),
        "fallback preview includes the manager; got: {lines:?}"
    );
}

#[test]
fn tree_uses_display_name_over_id() {
    // A line for an agent with `display_name` shows the friendly label,
    // not the raw id.
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss:
    display_name: The Captain
workers: {}
";
    let files = tree_files(COMPOSE_ONE_DEMO, &[("projects/demo.yaml", project)]);
    let lines = plain_tree(&files);
    assert!(
        lines.iter().any(|l| l.contains("The Captain")),
        "tree line shows display_name; got: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("boss")),
        "tree line should not fall back to the id when display_name is set; got: {lines:?}"
    );
}

#[test]
fn tree_line_carries_agent_descriptor() {
    // Each agent row carries the grey runtime/model/counts descriptor.
    // Default runtime is claude-code → "Claude Code", and the counts
    // suffix is always present.
    let project = "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss: {}
workers: {}
";
    let files = tree_files(COMPOSE_ONE_DEMO, &[("projects/demo.yaml", project)]);
    let lines = plain_tree(&files);
    let boss = lines
        .iter()
        .find(|l| l.contains("boss"))
        .expect("boss present");
    assert!(
        boss.contains("Claude Code") && boss.contains("×a"),
        "manager row carries the runtime + counts descriptor; got: {boss:?}"
    );
}

#[test]
fn tree_essentials_template_emits_single_root() {
    // The real `essentials` template ships an agentless `main` (skipped)
    // plus an `ops` project with one agent → exactly one `You`. Locks
    // the single-root behaviour (and flags the multi-root latent edge
    // the code comments call out: today every shipped template lands
    // here with one root).
    let essentials = TEMPLATES
        .iter()
        .find(|t| t.key == "essentials")
        .expect("essentials template");
    let lines = plain_tree(&essentials.entries());
    assert_eq!(
        lines.iter().filter(|l| *l == "You").count(),
        1,
        "essentials previews exactly one team root; got: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("ops")),
        "essentials preview includes the ops agent; got: {lines:?}"
    );
}

// ── lead_attach_target ────────────────────────────────────────────

/// Build a one-project `Compose` from a parsed project body.
fn compose_with(project: team_core::compose::Project) -> team_core::compose::Compose {
    let global =
        serde_yaml::from_str::<team_core::compose::Global>("version: \"2.0.0\"\nprojects: []\n")
            .expect("minimal global parses");
    team_core::compose::Compose {
        root: std::path::PathBuf::from("."),
        global,
        projects: vec![project],
    }
}

#[test]
fn lead_attach_prefers_the_manager_workers_report_to() {
    // Two managers; only `boss` is reported to. The lead-attach pick
    // must be `boss`, not the back-channel `compass` manager.
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss: {}
  compass: {}
workers:
  hand:
    reports_to: boss
",
    );
    let compose = compose_with(project);
    assert_eq!(
        lead_attach_target(&compose).as_deref(),
        Some("demo:boss"),
        "the manager workers report to wins the attach pick"
    );
}

#[test]
fn lead_attach_falls_back_to_first_manager_when_none_reported_to() {
    // No worker reports to anyone → fall back to a manager, the first
    // in `agents()` (managers-first, BTreeMap-sorted) order.
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  ay_mgr: {}
  zee_mgr: {}
workers: {}
",
    );
    let compose = compose_with(project);
    assert_eq!(
        lead_attach_target(&compose).as_deref(),
        Some("demo:ay_mgr"),
        "no reports_to → first manager in agents() order"
    );
}

#[test]
fn lead_attach_falls_back_to_any_agent_when_no_managers() {
    // Only workers exist → fall back to the first agent overall.
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers: {}
workers:
  ay_worker: {}
  zee_worker: {}
",
    );
    let compose = compose_with(project);
    assert_eq!(
        lead_attach_target(&compose).as_deref(),
        Some("demo:ay_worker"),
        "no managers → first agent overall"
    );
}

#[test]
fn lead_attach_none_when_no_agents() {
    // Agentless project (e.g. blank) → no attach hint at all.
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers: {}
workers: {}
",
    );
    let compose = compose_with(project);
    assert_eq!(
        lead_attach_target(&compose),
        None,
        "no agents → None (caller omits attach/bot hints)"
    );
}

#[test]
fn lead_attach_scopes_reports_to_by_project() {
    // Two agent-bearing projects each ship a manager named `boss`.
    // `solo` (listed FIRST) has no workers; `team` (second) has a
    // worker reporting to its own `boss`. The pick must be `team:boss`.
    // Without scoping the reports_to check by project, the first-listed
    // `solo:boss` would falsely match `team`'s worker by bare-name
    // collision and win — this pins the project-scoped behaviour.
    let solo = parse_project(
        "\
version: 2
project:
  id: solo
  name: Solo
  cwd: ./workspace
managers:
  boss: {}
workers: {}
",
    );
    let team = parse_project(
        "\
version: 2
project:
  id: team
  name: Team
  cwd: ./workspace
managers:
  boss: {}
workers:
  hand:
    reports_to: boss
",
    );
    let global =
        serde_yaml::from_str::<team_core::compose::Global>("version: \"2.0.0\"\nprojects: []\n")
            .expect("minimal global parses");
    let compose = team_core::compose::Compose {
        root: std::path::PathBuf::from("."),
        global,
        projects: vec![solo, team],
    };
    assert_eq!(
        lead_attach_target(&compose).as_deref(),
        Some("team:boss"),
        "the reported-to manager in the worker's OWN project wins, not a same-named manager elsewhere"
    );
}

// ── agent_descriptor / runtime_label / model_label / label_for ────

#[test]
fn agent_descriptor_all_zero_counts() {
    // Minimal agent: default runtime, no model, every count zero.
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss: {}
workers: {}
",
    );
    let agent = &project.managers["boss"];
    assert_eq!(agent_descriptor(agent), "Claude Code · 0×a 0×s 0×h 0×m");
}

#[test]
fn agent_descriptor_counts_and_model_segment() {
    // subagents/skills populated, a pinned model, default runtime. The
    // model segment slots between runtime and counts; counts reflect
    // each Vec's len (hooks/mcps stay 0 here — see the all-zero case
    // for the formatter and the per-field zero).
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss:
    model: claude-opus-4-8
    subagents: [a.md, b.md, c.md]
    skills: [skills/one]
workers: {}
",
    );
    let agent = &project.managers["boss"];
    assert_eq!(
        agent_descriptor(agent),
        "Claude Code · Opus 4.8 · 3×a 1×s 0×h 0×m",
        "descriptor = runtime · model · counts, joined by ` · `"
    );
}

#[test]
fn agent_descriptor_omits_model_when_unset() {
    // No model pinned → the descriptor has exactly two ` · ` segments
    // (runtime, counts) with no empty model slot.
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  boss:
    subagents: [a.md]
workers: {}
",
    );
    let agent = &project.managers["boss"];
    let desc = agent_descriptor(agent);
    assert_eq!(desc, "Claude Code · 1×a 0×s 0×h 0×m");
    assert_eq!(
        desc.matches(" · ").count(),
        1,
        "no model → no model segment; got: {desc:?}"
    );
}

#[test]
fn runtime_label_known_and_unknown() {
    assert_eq!(runtime_label("claude-code"), "Claude Code");
    // Unknown runtimes pass through verbatim.
    assert_eq!(runtime_label("codex"), "codex");
    assert_eq!(runtime_label(""), "");
}

#[test]
fn model_label_all_arms_including_both_haiku_aliases() {
    assert_eq!(model_label("claude-opus-4-8"), "Opus 4.8");
    assert_eq!(model_label("claude-sonnet-4-6"), "Sonnet 4.6");
    assert_eq!(model_label("claude-haiku-4-5"), "Haiku 4.5");
    assert_eq!(
        model_label("claude-haiku-4-5-20251001"),
        "Haiku 4.5",
        "the dated haiku alias maps to the same label"
    );
    // Anything unrecognized is shown raw.
    assert_eq!(model_label("gpt-5"), "gpt-5");
}

#[test]
fn label_for_prefers_display_name_else_id() {
    let project = parse_project(
        "\
version: 2
project:
  id: demo
  name: Demo
  cwd: ./workspace
managers:
  named:
    display_name: The Captain
  bare: {}
workers: {}
",
    );
    assert_eq!(
        label_for("named", &project.managers["named"]),
        "The Captain",
        "display_name wins when set"
    );
    assert_eq!(
        label_for("bare", &project.managers["bare"]),
        "bare",
        "falls back to the id when display_name is absent"
    );
}

// ── TreeNode (scaffold file-list renderer) ────────────────────────

/// Render a TreeNode into a `Vec<String>` for assertion.
fn render_tree(tree: &TreeNode) -> Vec<String> {
    let mut out = Vec::new();
    tree.render("", &mut |line| out.push(line));
    out
}

#[test]
fn treenode_renders_dirs_files_and_connectors() {
    // a/b.txt, a/c.txt, d.txt → `a/` is a dir (trailing slash) with
    // b.txt/c.txt nested under it via tee/elbow + a continuation
    // column; d.txt is a top-level leaf with NO slash.
    let mut tree = TreeNode::default();
    tree.insert(&["a", "b.txt"]);
    tree.insert(&["a", "c.txt"]);
    tree.insert(&["d.txt"]);
    let lines = render_tree(&tree);

    assert_eq!(
        lines,
        vec![
            "├── a/".to_string(),
            "│   ├── b.txt".to_string(),
            "│   └── c.txt".to_string(),
            "└── d.txt".to_string(),
        ],
        "dir gets a trailing slash; leaves don't; connectors + continuation match"
    );
}

#[test]
fn treenode_preserves_insertion_order_not_alpha() {
    // Children render first-seen, NOT sorted: insert z.txt before
    // a.txt and z.txt must still appear first.
    let mut tree = TreeNode::default();
    tree.insert(&["z.txt"]);
    tree.insert(&["a.txt"]);
    let lines = render_tree(&tree);
    assert_eq!(
        lines,
        vec!["├── z.txt".to_string(), "└── a.txt".to_string()],
        "insertion order is preserved (z before a), not alpha-sorted"
    );
}

#[test]
fn treenode_empty_renders_nothing() {
    let tree = TreeNode::default();
    assert!(
        render_tree(&tree).is_empty(),
        "an empty tree emits no lines"
    );
}

#[test]
fn treenode_inserting_existing_path_does_not_duplicate() {
    // Inserting the same intermediate dir twice reuses the node rather
    // than creating a second `a/` — the file list must show each dir
    // once even when many files live under it.
    let mut tree = TreeNode::default();
    tree.insert(&["a", "b.txt"]);
    tree.insert(&["a", "c.txt"]);
    assert_eq!(
        tree.children.len(),
        1,
        "the shared `a/` dir is a single node, not duplicated"
    );
}
