//! `teamctl exec <agent> -- <cmd>` and `teamctl shell <agent>`.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};

pub fn run(root: &Path, target: &str, argv: &[String]) -> Result<()> {
    if argv.is_empty() {
        bail!("usage: teamctl exec <agent> -- <command> [args...]");
    }
    let (bin, env) = setup(root, target)?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.current_dir(&bin);
    for (k, v) in &env {
        cmd.env(k, v);
    }
    let st = cmd.status()?;
    if let Some(code) = st.code() {
        std::process::exit(code);
    }
    Ok(())
}

pub fn shell(root: &Path, target: &str) -> Result<()> {
    let (cwd, env) = setup(root, target)?;
    let shell_bin = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut cmd = Command::new(&shell_bin);
    cmd.current_dir(&cwd);
    for (k, v) in &env {
        cmd.env(k, v);
    }
    eprintln!(
        "[teamctl shell] entering {} with env from {target}; ^D to exit",
        cwd.display()
    );
    let st = cmd.status()?;
    if let Some(code) = st.code() {
        std::process::exit(code);
    }
    Ok(())
}

/// Returns `(cwd, env_pairs)` for the agent: cwd from compose, env from the
/// rendered `state/envs/<project>-<agent>.env`.
fn setup(root: &Path, target: &str) -> Result<(std::path::PathBuf, Vec<(String, String)>)> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == handle.project)
        .expect("project resolved");
    let cwd = compose.root.join(&project.project.cwd);
    let env_file = team_core::render::env_path(&compose.root, handle.project, handle.agent);
    let mut env = Vec::new();
    if env_file.exists() {
        let raw = std::fs::read_to_string(&env_file)?;
        for line in raw.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                env.push((k.into(), v.into()));
            }
        }
    } else {
        eprintln!(
            "warning: {} not found — run `teamctl up` first to render env files",
            env_file.display()
        );
    }
    Ok((cwd, env))
}
