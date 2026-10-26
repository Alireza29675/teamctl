//! Scaffold a portable .team directory with an embedded personal-agent recipe.
//! Personal is the default for interactive and --yes use. Older explicit
//! templates remain compatibility paths. The browser workspace is the primary
//! messaging and inspection surface for new teams.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use include_dir::{include_dir, Dir};

mod picker;
mod preview;
mod templates;

use picker::*;
use preview::*;
use templates::{picker_menu, template_keys, Choice};
#[allow(unused_imports)] // Stable template facade used by tests and future CLI helpers.
pub use templates::{template_list_for_cli, Template, GUIDED_KEY, TEMPLATES};
#[cfg(test)]
use templates::{COMMON_ENV, COMMON_GITIGNORE};

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
            Choice::Template(TEMPLATES.iter().find(|t| t.key == "personal").unwrap())
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
    fs::create_dir_all(parent.join("workspace"))?;
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
    let _attach_target = super::load(&target)
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
    println!("  {:<30}# start the agents", "teamctl up");
    println!("  {:<30}# open chat and inspection", "teamctl serve");
    Ok(())
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

#[cfg(test)]
mod tests;
