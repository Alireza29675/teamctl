//! `teamctl init [name] [--template <name>] [--project <id>] [--force] [--yes]`
//!
//! Scaffold a `.team/` directory. With `name`, creates `<cwd>/<name>/.team/`
//! so a fresh `cd <name> && teamctl up` Just Works. Without `name`,
//! scaffolds `.team/` directly in cwd (the legacy in-place flow).
//!
//! Three templates:
//!
//! - `guided`     — ships no files; execs `claude /teamctl:init` so the
//!   LLM-led conversational setup takes over.
//! - `essentials` — two projects: a blank `main` for the operator + an
//!   `ops` project with a `builder` agent that helps evolve `main` over
//!   time.
//! - `blank`      — empty compose tree for operators who know exactly
//!   what they want.
//!
//! Templates are baked into the binary via `include_str!` so `init` works
//! offline. When run interactively (no `--yes`), the user picks a
//! template and confirms; the picker defaults to Guided. With `--yes`,
//! the default is `essentials` — `guided` requires the interactive
//! confirmation step and `--template guided --yes` errors clearly.
//! `--force` overwrites an existing `.team/` at the target path.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

#[derive(Clone, Copy)]
pub struct Template {
    pub key: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// Sentinel key for the `guided` template. It ships no files; selecting
/// it execs `claude /teamctl:init` instead of writing a tree, so we
/// keep it out of `TEMPLATES` and special-case the dispatch.
pub const GUIDED_KEY: &str = "guided";

/// File-shipping templates. Order matches picker display order
/// (Essentials first, then Blank). `guided` is shown above these in
/// the picker but handled out-of-band — see `choose_template_interactive`.
pub const TEMPLATES: &[Template] = &[
    Template {
        key: "essentials",
        label: "Essentials",
        blurb: "A blank project + a builder bot that helps you evolve it.",
        files: &[
            (
                "team-compose.yaml",
                include_str!("../../assets/templates/essentials/team-compose.yaml"),
            ),
            (
                "projects/main.yaml",
                include_str!("../../assets/templates/essentials/projects/main.yaml"),
            ),
            (
                "projects/ops.yaml",
                include_str!("../../assets/templates/essentials/projects/ops.yaml"),
            ),
            (
                "roles/builder.md",
                include_str!("../../assets/templates/essentials/roles/builder.md"),
            ),
            (
                ".env.example",
                include_str!("../../assets/templates/essentials/.env.example"),
            ),
            (
                ".gitignore",
                include_str!("../../assets/templates/_common/.gitignore"),
            ),
            (
                "README.md",
                include_str!("../../assets/templates/essentials/README.md"),
            ),
        ],
    },
    Template {
        key: "blank",
        label: "Blank",
        blurb: "Empty compose tree. Wire it up yourself.",
        files: &[
            (
                "team-compose.yaml",
                include_str!("../../assets/templates/blank/team-compose.yaml"),
            ),
            (
                "projects/main.yaml",
                include_str!("../../assets/templates/blank/projects/main.yaml"),
            ),
            (
                ".env.example",
                include_str!("../../assets/templates/_common/.env.example"),
            ),
            (
                ".gitignore",
                include_str!("../../assets/templates/_common/.gitignore"),
            ),
        ],
    },
];

/// The result of picker dispatch. `Guided` is the exec-claude path;
/// `Template` is the scaffold-files path.
enum Choice {
    Guided,
    Template(&'static Template),
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

    if !yes {
        eprintln!();
        eprintln!("About to scaffold `.team/` at {}:", target.display());
        eprintln!("  template:    {} ({})", tpl.label, tpl.key);
        eprintln!("  project id:  {pid}");
        eprintln!("  files:");
        for (path, _) in tpl.files {
            eprintln!("    .team/{path}");
        }
        if !confirm("Proceed?")? {
            bail!("aborted");
        }
    }

    fs::create_dir_all(&target)?;
    for (relpath, contents) in tpl.files {
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
    println!("Next:");
    if name.is_some() {
        let display = name.as_deref().unwrap_or(".");
        println!("  cd {display}");
    }
    println!("  cp .team/.env.example .team/.env   # edit secrets");
    println!("  teamctl validate                   # sanity-check");
    println!("  teamctl up                         # start the team");
    Ok(())
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

/// Picker UX. Shows Guided / Essentials / Blank in that order with
/// Guided as the default (Enter selects it). Returns a `Choice` so
/// the caller can branch to exec-claude vs. file-scaffold paths.
///
/// On Guided selection, the confirm-intent prompt is handled by the
/// caller (so the picker is pure-input → pure-output). That keeps
/// this function testable without piping a `claude` mock.
fn choose_template_interactive() -> Result<Choice> {
    eprintln!("Pick a template:");
    eprintln!(
        "  1) {:<14} — LLM walks you through setup (opens Claude Code)",
        "Guided"
    );
    for (i, t) in TEMPLATES.iter().enumerate() {
        // 1) is Guided, 2..) are the file-shipping templates.
        eprintln!("  {}) {:<14} — {}", i + 2, t.label, t.blurb);
    }
    eprint!("Choice [1]: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(parse_picker_input(line.trim()))
}

/// Pure-function picker dispatch. Empty input → Guided (default-on-Enter).
/// "1" → Guided. "2" → first TEMPLATES entry (Essentials). "3" → second
/// TEMPLATES entry (Blank). Anything unparseable or out-of-range falls
/// back to Guided so accidental keystrokes land on the most-supported
/// path rather than the bare-tree one.
fn parse_picker_input(trimmed: &str) -> Choice {
    if trimmed.is_empty() {
        return Choice::Guided;
    }
    match trimmed.parse::<usize>() {
        Ok(1) => Choice::Guided,
        Ok(n) => {
            // n=2 → TEMPLATES[0], n=3 → TEMPLATES[1], ...
            let idx = n.saturating_sub(2);
            TEMPLATES
                .get(idx)
                .map(Choice::Template)
                .unwrap_or(Choice::Guided)
        }
        Err(_) => Choice::Guided,
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
    fn picker_empty_input_defaults_to_guided() {
        // T-206: Enter-with-no-input lands the operator on the
        // most-supported path. Picking the bare-tree variant on a
        // stray keystroke would be unfriendly.
        assert!(matches!(parse_picker_input(""), Choice::Guided));
    }

    #[test]
    fn picker_one_selects_guided() {
        assert!(matches!(parse_picker_input("1"), Choice::Guided));
    }

    #[test]
    fn picker_two_selects_essentials() {
        // Essentials is the first entry in `TEMPLATES` (Guided lives
        // out-of-band, displayed as `1` in the picker UI).
        match parse_picker_input("2") {
            Choice::Template(t) => assert_eq!(t.key, "essentials"),
            _ => panic!("expected Choice::Template(essentials) for input `2`"),
        }
    }

    #[test]
    fn picker_three_selects_blank() {
        match parse_picker_input("3") {
            Choice::Template(t) => assert_eq!(t.key, "blank"),
            _ => panic!("expected Choice::Template(blank) for input `3`"),
        }
    }

    #[test]
    fn picker_out_of_range_falls_back_to_guided() {
        assert!(matches!(parse_picker_input("99"), Choice::Guided));
        assert!(matches!(parse_picker_input("hello"), Choice::Guided));
    }

    #[test]
    fn template_keys_lists_guided_first() {
        // Error-message ordering matters: Guided is the picker-default
        // and should be the first option the operator reads when they
        // mistype `--template`.
        let keys = template_keys();
        assert!(
            keys.starts_with("guided"),
            "expected `guided` to lead; got `{keys}`"
        );
        assert!(keys.contains("essentials"));
        assert!(keys.contains("blank"));
    }
}
