//! `teamctl init [name] [--template <name>] [--project <id>] [--force] [--yes]`
//!
//! Scaffold a `.team/` directory. With `name`, creates `<cwd>/<name>/.team/`
//! so a fresh `cd <name> && teamctl up` Just Works. Without `name`,
//! scaffolds `.team/` directly in cwd (the legacy in-place flow).
//!
//! Four templates:
//!
//! - `ideate-and-build` — a four-agent team (an Executor, a Compass
//!   ideation partner, and two engineers) that thinks an idea through,
//!   then builds it. The flagship; the interactive picker defaults to it.
//! - `guided`     — ships no files; execs `claude /teamctl:init` so the
//!   LLM-led conversational setup takes over.
//! - `essentials` — two projects: a blank `main` for the operator + an
//!   `ops` project with an `ops` agent that helps evolve `main` over
//!   time.
//! - `blank`      — empty compose tree for operators who know exactly
//!   what they want.
//!
//! Templates are baked into the binary via `include_dir!` (each template
//! is an embedded folder) so `init` works offline; the `_common/` shared
//! files are kept explicit per template so they stay single-source. When
//! run interactively (no `--yes`), the user picks a
//! template and confirms; the picker shows Ideate & Build / Guided /
//! Blank and defaults to Ideate & Build. `essentials` is intentionally
//! hidden from the picker but stays reachable via `--template essentials`
//! and remains the `--yes` default — `guided` requires the interactive
//! confirmation step and `--template guided --yes` errors clearly.
//! `--force` overwrites an existing `.team/` at the target path.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use include_dir::{include_dir, Dir};

#[derive(Clone, Copy)]
pub struct Template {
    pub key: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    /// The template's own file tree, embedded at compile time. Walked
    /// recursively at materialize time; each file's path relative to the
    /// embedded root becomes its destination relpath under `.team/`.
    /// Adding a file to the folder needs no edit here.
    pub dir: &'static Dir<'static>,
    /// Shared `_common/` overlays that don't live in the template's own
    /// folder, as `(dest relpath, contents)`. Kept explicit so `_common`
    /// stays single-source rather than copied into each template folder.
    pub shared: &'static [(&'static str, &'static str)],
}

impl Template {
    /// Materialized file set: every file in `dir` (recursively, by
    /// root-relative path) plus the `shared` overlays. Skips OS metadata
    /// files (see `collect_dir`). Ordered deterministically so the
    /// non-`--yes` dry-run preview reads naturally — `team-compose.yaml`
    /// (the file that defines the team) leads, then everything else
    /// alphabetically (so the dotfiles group together rather than the
    /// `shared` overlays dangling at the end). The materialized tree is
    /// identical regardless of order.
    pub(crate) fn entries(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Vec::new();
        collect_dir(self.dir, &mut out);
        out.extend(self.shared.iter().map(|(p, c)| ((*p).to_string(), *c)));
        out.sort_by(|a, b| {
            let rank = |p: &str| usize::from(p != "team-compose.yaml");
            rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0))
        });
        out
    }
}

/// Recursively collect `(root-relative path, utf8 contents)` for every
/// file under `dir`, skipping OS-injected directory-metadata files.
/// `include_dir` yields paths relative to the embedded root, which are
/// exactly the destination relpaths under `.team/`.
fn collect_dir(dir: &'static Dir<'static>, out: &mut Vec<(String, &'static str)>) {
    for file in dir.files() {
        let rel = file.path().to_string_lossy().into_owned();
        // Never template content, but they embed at compile time if a
        // maintainer's working tree has them when `include_dir!` runs.
        if rel.ends_with(".DS_Store") || rel.ends_with("Thumbs.db") {
            continue;
        }
        let contents = file
            .contents_utf8()
            .unwrap_or_else(|| panic!("template file `{rel}` is not valid UTF-8"));
        out.push((rel, contents));
    }
    for sub in dir.dirs() {
        collect_dir(sub, out);
    }
}

static IDEATE_DIR: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/assets/templates/ideate-and-build");
static ESSENTIALS_DIR: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/assets/templates/essentials");
static BLANK_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/templates/blank");

/// `_common/` files shared across templates, embedded once so they stay
/// single-source. Referenced from each template's `shared` overlay under
/// the destination path that template expects.
const COMMON_GITIGNORE: &str = include_str!("../../assets/templates/_common/.gitignore");
const COMMON_ENV: &str = include_str!("../../assets/templates/_common/.env.example");

/// Sentinel key for the `guided` template. It ships no files; selecting
/// it execs `claude /teamctl:init` instead of writing a tree, so we
/// keep it out of `TEMPLATES` and special-case the dispatch.
pub const GUIDED_KEY: &str = "guided";

/// File-shipping templates. `guided` is interleaved into the picker
/// display but handled out-of-band — see `picker_menu` /
/// `choose_template_interactive`. This array holds every template
/// reachable via `--template` (including `essentials`, which is hidden
/// from the interactive picker); its order is independent of display
/// order, which `picker_menu` spells out explicitly.
pub const TEMPLATES: &[Template] = &[
    Template {
        key: "ideate-and-build",
        label: "Ideate & Build",
        blurb: "An Executor, a Compass ideation partner, and two engineers — think it through, then build it.",
        dir: &IDEATE_DIR,
        // Own folder ships roles/, agents/, charter.md, .env.example,
        // README.md; only .gitignore comes from _common.
        shared: &[(".gitignore", COMMON_GITIGNORE)],
    },
    Template {
        key: "essentials",
        label: "Essentials",
        blurb: "A blank project + an ops bot that helps you evolve it.",
        dir: &ESSENTIALS_DIR,
        // Own folder ships its .env.example; only .gitignore is shared.
        shared: &[(".gitignore", COMMON_GITIGNORE)],
    },
    Template {
        key: "blank",
        label: "Blank",
        blurb: "Empty compose tree. Wire it up yourself.",
        dir: &BLANK_DIR,
        // Bare tree — both dotfiles come from _common.
        shared: &[
            (".env.example", COMMON_ENV),
            (".gitignore", COMMON_GITIGNORE),
        ],
    },
];

/// The result of picker dispatch. `Guided` is the exec-claude path;
/// `Template` is the scaffold-files path.
#[derive(Clone, Copy)]
enum Choice {
    Guided,
    Template(&'static Template),
}

/// The picker menu in display order: Ideate & Build, Guided, Blank.
/// Guided is interleaved among the file-shipping templates, so the order
/// is spelled out here rather than derived from `TEMPLATES` iteration.
/// This is the single source of truth for both what the picker prints
/// and how a typed number maps to a choice.
///
/// Note: `essentials` is intentionally NOT shown in the interactive
/// picker (operator's call) — it stays in `TEMPLATES` so the
/// `--template essentials` flag and `--yes` default keep working, and
/// re-adding it to the menu is a one-line change here.
fn picker_menu() -> Vec<Choice> {
    let by_key = |k: &str| {
        TEMPLATES
            .iter()
            .find(|t| t.key == k)
            .unwrap_or_else(|| panic!("missing template `{k}` in TEMPLATES"))
    };
    vec![
        Choice::Template(by_key("ideate-and-build")),
        Choice::Guided,
        Choice::Template(by_key("blank")),
    ]
}

pub fn run(
    name: Option<String>,
    template: Option<String>,
    project_id: Option<String>,
    force: bool,
    yes: bool,
) -> Result<()> {
    // `guided` is interactive-only: it requires the confirm-intent
    // prompt, which can't run under `--yes`. Reject early with a
    // clear message rather than letting it slip past and confuse the
    // operator at exec time.
    if matches!(template.as_deref(), Some(GUIDED_KEY)) && yes {
        bail!(
            "`--template guided` requires interactive confirmation and is incompatible with \
             `--yes`. Drop `--yes` to run the guided flow, or pick `--template essentials` for \
             a non-interactive scaffold."
        );
    }

    let choice = match template {
        Some(k) if k == GUIDED_KEY => {
            // Explicit `--template guided` — confirm intent, then exec.
            // No scaffold, no project-id substitution.
            if !confirm("This will open Claude Code and run `/teamctl:init`. Continue?")? {
                bail!("aborted");
            }
            exec_guided()?;
            // exec_guided typically replaces the process; if we return
            // (claude exited normally without exec), bail with the same
            // shape so the caller sees a graceful completion.
            return Ok(());
        }
        Some(k) => Choice::Template(
            TEMPLATES
                .iter()
                .find(|t| t.key == k)
                .ok_or_else(|| anyhow!("unknown template `{k}` — known: {}", template_keys()))?,
        ),
        None if yes => {
            // Default in non-interactive mode is Essentials. Guided is
            // interactive-only (rejected above); Essentials gives the
            // operator a useful starting team without forcing a Claude
            // Code session.
            Choice::Template(TEMPLATES.iter().find(|t| t.key == "essentials").unwrap())
        }
        None => choose_template_interactive()?,
    };

    let tpl = match choice {
        Choice::Guided => {
            // Picker-selected guided: confirm + exec, same as the
            // explicit-flag branch above.
            if !confirm("This will open Claude Code and run `/teamctl:init`. Continue?")? {
                bail!("aborted");
            }
            exec_guided()?;
            return Ok(());
        }
        Choice::Template(t) => t,
    };

    let cwd = std::env::current_dir().context("get cwd")?;

    // Target layout:
    //   teamctl init my-team   →  <cwd>/my-team/.team/...   (`name` set)
    //   teamctl init           →  <cwd>/.team/...           (in-place)
    let (parent, target) = match &name {
        Some(n) => {
            let dir = cwd.join(n);
            (dir.clone(), dir.join(".team"))
        }
        None => (cwd.clone(), cwd.join(".team")),
    };

    if target.exists() {
        if force {
            fs::remove_dir_all(&target)
                .with_context(|| format!("--force: remove existing {}", target.display()))?;
        } else {
            bail!(
                "{} already exists. Pass --force to overwrite, or pick a different name.",
                target.display()
            );
        }
    }

    // Project id derives from (in order): explicit --project flag,
    // positional `name`, parent-directory basename. The slugify pass
    // is the same for all three so a name like "My Team!" still
    // produces a usable id.
    let pid = project_id.unwrap_or_else(|| {
        name.as_deref()
            .map(slugify)
            .filter(|s| !s.is_empty())
            .or_else(|| parent.file_name().and_then(|s| s.to_str()).map(slugify))
            .unwrap_or_else(|| "main".into())
    });

    let mut subs: BTreeMap<&str, String> = BTreeMap::new();
    subs.insert("project_id", pid.clone());
    subs.insert("project_name", titlecase(&pid));

    let files = tpl.entries();

    if !yes {
        // Show the team itself before the file list — same ordering as
        // the teamctl-ui Agents pane (managers first, then workers).
        // Best-effort: parses the in-memory template before anything is
        // written, and stays silent on a parse failure or an agentless
        // template (e.g. `blank`) rather than failing init.
        let team = team_structure_lines(&files);
        if !team.is_empty() {
            eprintln!();
            eprintln!("Team shape:");
            for line in &team {
                eprintln!("{line}");
            }
        }

        eprintln!();
        eprintln!("About to scaffold `.team/` at {}:", target.display());
        eprintln!("  template:    {} ({})", tpl.label, tpl.key);
        eprintln!("  project id:  {pid}");
        eprintln!("  files:");
        eprintln!("    .team/");
        let mut tree = TreeNode::default();
        for (path, _) in &files {
            tree.insert(&path.split('/').collect::<Vec<_>>());
        }
        tree.render("    ", &mut |line| eprintln!("{line}"));
        if !confirm("Proceed?")? {
            bail!("aborted");
        }
    }

    fs::create_dir_all(&target)?;
    for (relpath, contents) in &files {
        let dest = target.join(relpath);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let rendered = substitute(contents, &subs);
        fs::write(&dest, rendered)?;
    }

    println!();
    println!("✓ {} scaffolded.", target.display());
    println!();

    // The agent the operator most likely wants to attach to first — see
    // `lead_attach_target`. `None` for an agentless template (e.g.
    // `blank`), which omits the bot/attach hints in the `Next:` block.
    let attach_target = super::load(&target)
        .ok()
        .and_then(|c| lead_attach_target(&c));

    println!("Next:");
    if name.is_some() {
        let display = name.as_deref().unwrap_or(".");
        println!("  cd {display}");
    }
    println!(
        "  {:<30}# sanity-check the team structure",
        "teamctl validate"
    );
    if attach_target.is_some() {
        println!(
            "  {:<30}# (optional) connect your agents to Telegram",
            "teamctl bot setup"
        );
    }
    println!("  {:<30}# start the team", "teamctl up");
    if let Some(id) = attach_target {
        println!(
            "  {:<30}# drop into a live agent session",
            format!("teamctl attach {id}")
        );
    }
    Ok(())
}

/// The agent the operator most likely wants to attach to first: the lead
/// manager that workers report to (e.g. the Executor that fronts the team,
/// not a private back-channel manager like Compass), falling back to any
/// manager, then any agent at all. Returns the canonical `<project>:<agent>`
/// id `teamctl attach` expects, or `None` when the compose scaffolds no
/// agents (e.g. the `blank` template) — the caller then omits the
/// bot/attach hints in the `Next:` block.
fn lead_attach_target(compose: &team_core::compose::Compose) -> Option<String> {
    let agents: Vec<_> = compose.agents().collect();
    // A manager counts as "lead" when a worker IN THE SAME PROJECT reports
    // to it. The project scope matters once a compose ships more than one
    // agent-bearing project — without it, a manager could falsely match a
    // same-named worker's `reports_to` in a different project. Today's
    // templates each ship a single agent-bearing project, so this is
    // behaviour-identical for them; it just keeps the function correct if
    // a multi-project template lands later.
    let reported_to = |mgr: &team_core::compose::AgentHandle| {
        agents
            .iter()
            .any(|w| w.project == mgr.project && w.spec.reports_to.as_deref() == Some(mgr.agent))
    };
    agents
        .iter()
        .find(|a| a.is_manager && reported_to(a))
        .or_else(|| agents.iter().find(|a| a.is_manager))
        .or_else(|| agents.first())
        .map(|h| h.id())
}

/// Build the team-structure preview from the in-memory template files
/// (not yet on disk at preview time). `team-compose.yaml` names the
/// project files; each `projects/<name>.yaml` deserializes into a
/// `Project`. Managers render at the top of each project; workers nest
/// under the manager they `reports_to` (so a reader sees the actual
/// reporting hierarchy, not a flat list). A worker that reports to no
/// listed manager hangs at the top level. Labels prefer `display_name`,
/// falling back to the id. The tree connectors stay grey while agent
/// names render in light pink (the `You` root in bold pink) — but only
/// when stderr is a terminal; piped/redirected output is plain so it
/// stays clean in logs and tests. Box-drawing matches the file tree.
///
/// Best-effort and non-fatal: a template with no parseable agents (e.g.
/// `blank`, or a compose we can't deserialize) yields an empty `Vec`,
/// and the caller simply skips the section. Init never fails on this.
fn team_structure_lines(files: &[(String, &'static str)]) -> Vec<String> {
    // House convention (see warn.rs / release_notes.rs): raw ANSI gated
    // on `is_terminal()`, no colour when piped. The preview prints to
    // stderr, so that's the stream we test.
    let color = io::stderr().is_terminal();
    let paint = |code: &str, s: &str| -> String {
        if color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    const GREY: &str = "90"; // tree lines + the `(role)` suffix
    const PINK: &str = "38;5;218"; // agent names — light pink (256-colour)

    let by_path = |p: &str| files.iter().find(|(rel, _)| rel == p).map(|(_, c)| *c);

    // The project file relpaths to render, in compose-declared order.
    // If `team-compose.yaml` is missing or unparseable, fall back to any
    // `projects/*.yaml` the template ships so a hand-rolled tree still
    // previews.
    let project_paths: Vec<String> = by_path("team-compose.yaml")
        .and_then(|c| serde_yaml::from_str::<team_core::compose::Global>(c).ok())
        .map(|g| {
            g.projects
                .iter()
                .map(|p| p.file.to_string_lossy().into_owned())
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| {
            files
                .iter()
                .map(|(rel, _)| rel.clone())
                .filter(|rel| rel.starts_with("projects/") && rel.ends_with(".yaml"))
                .collect()
        });

    let mut out = Vec::new();
    for path in project_paths {
        let Some(project) = by_path(&path)
            .and_then(|c| serde_yaml::from_str::<team_core::compose::Project>(c).ok())
        else {
            continue;
        };
        if project.managers.is_empty() && project.workers.is_empty() {
            continue;
        }

        // Group workers under the manager they report to; any worker
        // whose `reports_to` isn't a manager in this project hangs at
        // the top level. BTreeMap iteration keeps each group id-sorted.
        let mut children: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
        let mut orphans: Vec<&String> = Vec::new();
        for (wid, w) in &project.workers {
            match w.reports_to.as_deref() {
                Some(m) if project.managers.contains_key(m) => {
                    children.entry(m).or_default().push(wid)
                }
                _ => orphans.push(wid),
            }
        }

        // Top level: every manager (id-sorted), then any orphan workers.
        let top: Vec<(&String, &str)> = project
            .managers
            .keys()
            .map(|id| (id, "manager"))
            .chain(orphans.iter().map(|id| (*id, "worker")))
            .collect();

        // Flush-left (no leading indent), whole tree in light pink with
        // grey connectors + a grey runtime/model/counts descriptor. The
        // root is "You" — the operator — so the tree reads as a reporting
        // hierarchy (managers report to you; workers to their manager).
        // (Current templates each ship one agent-bearing project, so this
        // emits a single "You"; a future multi-agent-project template
        // would want one shared "You" — flagged for the hardening pass.)
        out.push(paint(&format!("1;{PINK}"), "You"));

        let row = |indent: &str, connector: &str, label: &str, descriptor: &str| {
            format!(
                "{}{}{}  {}",
                indent,
                paint(GREY, connector),
                paint(PINK, label),
                paint(GREY, descriptor),
            )
        };

        let last_top = top.len().saturating_sub(1);
        for (i, (id, _role)) in top.iter().enumerate() {
            let agent = project
                .managers
                .get(*id)
                .or_else(|| project.workers.get(*id));
            let label = agent.map_or_else(|| (*id).to_string(), |a| label_for(id, a));
            let descriptor = agent.map(agent_descriptor).unwrap_or_default();
            let top_last = i == last_top;
            out.push(row(
                "",
                if top_last { "└── " } else { "├── " },
                &label,
                &descriptor,
            ));

            // Workers reporting to this manager, nested one level in. The
            // continuation column is a grey `│` unless this manager is the
            // last top-level entry, in which case it's blank.
            let kids = children.get(id.as_str()).cloned().unwrap_or_default();
            let cont = if top_last {
                "    ".to_string()
            } else {
                format!("{}   ", paint(GREY, "│"))
            };
            let last_kid = kids.len().saturating_sub(1);
            for (j, wid) in kids.iter().enumerate() {
                let w = project.workers.get(wid.as_str());
                let label = w.map_or_else(|| (*wid).to_string(), |a| label_for(wid, a));
                let descriptor = w.map(agent_descriptor).unwrap_or_default();
                let conn = if j == last_kid {
                    "└── "
                } else {
                    "├── "
                };
                out.push(row(&cont, conn, &label, &descriptor));
            }
        }
    }
    out
}

/// Roster label for an agent: its `display_name` when set, else the id —
/// the same fallback the teamctl-ui roster uses.
fn label_for(id: &str, agent: &team_core::compose::Agent) -> String {
    agent.display_name.clone().unwrap_or_else(|| id.to_string())
}

/// One-line descriptor for an agent in the team tree: runtime, model (when
/// pinned), and counts of the per-agent settings — `N×a` sub-agents, `N×s`
/// skills, `N×h` hooks, `N×m` mcps. E.g. `Claude Code · 8×a 0×s 0×h 0×m`.
fn agent_descriptor(agent: &team_core::compose::Agent) -> String {
    let mut parts = vec![runtime_label(&agent.runtime)];
    if let Some(model) = &agent.model {
        parts.push(model_label(model));
    }
    parts.push(format!(
        "{}×a {}×s {}×h {}×m",
        agent.subagents.len(),
        agent.skills.len(),
        agent.hooks.len(),
        agent.mcps.len(),
    ));
    parts.join(" · ")
}

/// Human-friendly runtime name; unknown runtimes show their raw id.
fn runtime_label(runtime: &str) -> String {
    match runtime {
        "claude-code" => "Claude Code".to_string(),
        other => other.to_string(),
    }
}

/// Human-friendly model name for the known Claude ids; anything else shows
/// the raw model string the operator pinned.
fn model_label(model: &str) -> String {
    match model {
        "claude-opus-4-8" => "Opus 4.8".to_string(),
        "claude-sonnet-4-6" => "Sonnet 4.6".to_string(),
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => "Haiku 4.5".to_string(),
        other => other.to_string(),
    }
}

/// Run `claude /teamctl:init` to hand off to the LLM-led setup skill.
/// Inherits stdio so the operator sees the conversation in their
/// current terminal. If `claude` is not on PATH, surface a clear
/// error pointing at the install path.
fn exec_guided() -> Result<()> {
    let status = Command::new("claude")
        .arg("/teamctl:init")
        .status()
        .with_context(|| {
            "failed to launch `claude` — is Claude Code installed and on PATH? See \
             https://code.claude.com/docs"
        })?;
    if !status.success() {
        bail!(
            "`claude /teamctl:init` exited with status {status} — see the Claude Code output \
             above for details."
        );
    }
    Ok(())
}

/// One-line label + blurb for a menu slot, for the picker display.
fn choice_line(c: &Choice) -> (&'static str, &'static str) {
    match c {
        Choice::Guided => ("Guided", "LLM walks you through setup (opens Claude Code)"),
        Choice::Template(t) => (t.label, t.blurb),
    }
}

/// Picker UX. Shows Ideate & Build / Essentials / Guided / Blank in that
/// order with Ideate & Build as the default (Enter selects it). Returns a
/// `Choice` so the caller can branch to exec-claude vs. file-scaffold
/// paths.
///
/// On Guided selection, the confirm-intent prompt is handled by the
/// caller (so the picker is pure-input → pure-output). That keeps
/// this function testable without piping a `claude` mock.
fn choose_template_interactive() -> Result<Choice> {
    eprintln!("Pick a template:");
    for (i, c) in picker_menu().iter().enumerate() {
        let (label, blurb) = choice_line(c);
        eprintln!("  {}) {:<16} — {}", i + 1, label, blurb);
    }
    eprint!("Choice [1]: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(parse_picker_input(line.trim()))
}

/// Pure-function picker dispatch over `picker_menu()` display order
/// (1 = Ideate & Build, 2 = Essentials, 3 = Guided, 4 = Blank). Empty
/// input → slot 1 (default-on-Enter = the flagship). Anything unparseable
/// or out-of-range also falls back to slot 1 so accidental keystrokes
/// land on the most-supported path rather than the bare-tree one.
fn parse_picker_input(trimmed: &str) -> Choice {
    let menu = picker_menu();
    if trimmed.is_empty() {
        return menu[0];
    }
    match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 => menu.get(n - 1).copied().unwrap_or(menu[0]),
        _ => menu[0],
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [Y/n] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(answer_is_yes(&line))
}

/// Parse a y/N answer with **Yes** as the empty-input default: a bare
/// Enter proceeds. Pulled out of `confirm` so the default can be unit
/// tested without driving real stdin.
fn answer_is_yes(line: &str) -> bool {
    let s = line.trim().to_lowercase();
    s.is_empty() || s == "y" || s == "yes"
}

fn template_keys() -> String {
    // `guided` ships no files but is a valid --template value; list it
    // alongside the file-shipping templates so the error message
    // matches what the CLI actually accepts.
    let mut keys: Vec<&str> = vec![GUIDED_KEY];
    keys.extend(TEMPLATES.iter().map(|t| t.key));
    keys.join(", ")
}

fn substitute(s: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn titlecase(s: &str) -> String {
    s.split('-')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A node in the scaffold file tree, used to render the `teamctl init`
/// confirmation as a `tree`-style hierarchy instead of a flat path list.
/// Children preserve first-seen (template) order; a node with children is
/// rendered as a directory (trailing `/`), a leaf as a file.
#[derive(Default)]
struct TreeNode {
    children: Vec<(String, TreeNode)>,
}

impl TreeNode {
    /// Insert a path's `/`-split components, creating intermediate nodes.
    fn insert(&mut self, parts: &[&str]) {
        let Some((head, rest)) = parts.split_first() else {
            return;
        };
        let idx = match self.children.iter().position(|(n, _)| n == head) {
            Some(i) => i,
            None => {
                self.children
                    .push(((*head).to_string(), TreeNode::default()));
                self.children.len() - 1
            }
        };
        self.children[idx].1.insert(rest);
    }

    /// Emit each line via `out`, prefixing with box-drawing connectors.
    fn render(&self, prefix: &str, out: &mut impl FnMut(String)) {
        let last_idx = self.children.len().saturating_sub(1);
        for (i, (name, child)) in self.children.iter().enumerate() {
            let last = i == last_idx;
            let connector = if last { "└── " } else { "├── " };
            let slash = if child.children.is_empty() { "" } else { "/" };
            out(format!("{prefix}{connector}{name}{slash}"));
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            child.render(&child_prefix, out);
        }
    }
}

#[allow(dead_code)]
pub fn template_list_for_cli() -> Vec<&'static str> {
    let mut keys: Vec<&str> = vec![GUIDED_KEY];
    keys.extend(TEMPLATES.iter().map(|t| t.key));
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "agents/implementer.md",
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
        let mut files: Vec<(String, &'static str)> =
            vec![("team-compose.yaml".to_string(), compose)];
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
        let global = serde_yaml::from_str::<team_core::compose::Global>(
            "version: \"2.0.0\"\nprojects: []\n",
        )
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
        let global = serde_yaml::from_str::<team_core::compose::Global>(
            "version: \"2.0.0\"\nprojects: []\n",
        )
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
}
