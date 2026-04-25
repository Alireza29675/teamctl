//! `teamctl init [--template <name>] [--yes]`
//!
//! Scaffold a `.team/` directory in the current repo. Templates are baked
//! into the binary so `init` works offline and produces consistent output.
//! When run interactively (no `--yes`), the user picks a template, names
//! the project, and confirms.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};

use anyhow::{anyhow, bail, Context, Result};

#[derive(Clone, Copy)]
pub struct Template {
    pub key: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

pub const TEMPLATES: &[Template] = &[
    Template {
        key: "solo",
        label: "Solo team",
        blurb: "One manager + one dev, both on Claude Code. Smallest useful team.",
        files: &[
            (
                "team-compose.yaml",
                include_str!("../../assets/templates/solo/team-compose.yaml"),
            ),
            (
                "projects/main.yaml",
                include_str!("../../assets/templates/solo/projects/main.yaml"),
            ),
            (
                "roles/manager.md",
                include_str!("../../assets/templates/solo/roles/manager.md"),
            ),
            (
                "roles/dev.md",
                include_str!("../../assets/templates/solo/roles/dev.md"),
            ),
            (
                ".env.example",
                include_str!("../../assets/templates/solo/.env.example"),
            ),
            (
                ".gitignore",
                include_str!("../../assets/templates/_common/.gitignore"),
            ),
            (
                "README.md",
                include_str!("../../assets/templates/solo/README.md"),
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

pub fn run(template: Option<String>, project_id: Option<String>, yes: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("get cwd")?;
    let target = cwd.join(".team");
    if target.exists() {
        bail!(
            "{} already exists. Remove it or pick a different directory.",
            target.display()
        );
    }

    let tpl = match template {
        Some(k) => TEMPLATES
            .iter()
            .find(|t| t.key == k)
            .ok_or_else(|| anyhow!("unknown template `{k}` — known: {}", template_keys()))?,
        None if yes => {
            // Default in non-interactive mode.
            TEMPLATES.iter().find(|t| t.key == "solo").unwrap()
        }
        None => choose_template_interactive()?,
    };

    let pid = project_id.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|s| s.to_str())
            .map(slugify)
            .unwrap_or_else(|| "main".into())
    });

    let mut subs: BTreeMap<&str, String> = BTreeMap::new();
    subs.insert("project_id", pid.clone());
    subs.insert("project_name", titlecase(&pid));

    if !yes {
        eprintln!();
        eprintln!("About to scaffold `.team/` in {}:", cwd.display());
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
    println!("  cp .team/.env.example .team/.env   # edit secrets");
    println!("  teamctl validate                   # sanity-check");
    println!("  teamctl up                         # start the team");
    Ok(())
}

fn choose_template_interactive() -> Result<&'static Template> {
    eprintln!("Pick a template:");
    for (i, t) in TEMPLATES.iter().enumerate() {
        eprintln!("  {}) {:<14} — {}", i + 1, t.label, t.blurb);
    }
    eprint!("Choice [1]: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim();
    let idx = if trimmed.is_empty() {
        0
    } else {
        trimmed
            .parse::<usize>()
            .map(|n| n.saturating_sub(1))
            .unwrap_or(0)
    };
    Ok(TEMPLATES.get(idx).unwrap_or(&TEMPLATES[0]))
}

fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [Y/n] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let s = line.trim().to_lowercase();
    Ok(s.is_empty() || s == "y" || s == "yes")
}

fn template_keys() -> String {
    TEMPLATES
        .iter()
        .map(|t| t.key)
        .collect::<Vec<_>>()
        .join(", ")
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
    TEMPLATES.iter().map(|t| t.key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
