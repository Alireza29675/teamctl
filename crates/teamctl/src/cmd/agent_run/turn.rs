use super::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

pub(super) struct TurnResult {
    pub text: String,
    pub session: Option<String>,
}

pub(super) fn provider_args(
    runtime: &str,
    env: &BTreeMap<String, String>,
    session: Option<&str>,
) -> Result<Vec<String>> {
    let mut args: Vec<String> = Vec::new();
    let get = |key: &str| env.get(key).filter(|v| !v.is_empty());
    match runtime {
        "claude-code" => {
            args.extend([
                "--print".into(),
                "--output-format".into(),
                "json".into(),
                "--permission-prompts".into(),
                "none".into(),
                "--strict-mcp-config".into(),
            ]);
            if let Some(id) = session {
                args.extend(["--resume".into(), id.into()]);
            }
            if let Some(m) = get("MODEL") {
                args.extend(["--model".into(), m.clone()]);
            }
            if let Some(e) = get("EFFORT") {
                args.extend(["--effort".into(), e.clone()]);
            }
            args.extend([
                "--permission-mode".into(),
                get("PERMISSION_MODE")
                    .filter(|p| p.as_str() != "attended")
                    .cloned()
                    .unwrap_or_else(|| "auto".into()),
            ]);
            for (key, flag) in [
                ("MCP_CONFIG", "--mcp-config"),
                ("CLAUDE_SETTINGS", "--settings"),
                ("CLAUDE_AGENTS_JSON", "--agents"),
                ("CLAUDE_AGENT_SCOPE", "--add-dir"),
            ] {
                if let Some(path) = get(key).filter(|p| Path::new(p).exists()) {
                    args.extend([flag.into(), path.clone()]);
                }
            }
            if let Some(path) = get("SYSTEM_PROMPT_PATH") {
                let prompt = std::fs::read_to_string(path)?;
                args.extend(["--append-system-prompt".into(), prompt]);
            }
        }
        "codex" => {
            if get("PERMISSION_MODE").is_some_and(|p| p == "bypassPermissions") {
                args.extend(["--dangerously-bypass-approvals-and-sandbox".into()]);
            } else {
                args.extend([
                    "-a".into(),
                    "never".into(),
                    "-c".into(),
                    "sandbox_mode=\"workspace-write\"".into(),
                ]);
            }
            for (key, config) in [
                ("EFFORT", "model_reasoning_effort"),
                ("SYSTEM_PROMPT_PATH", "model_instructions_file"),
            ] {
                if let Some(v) = get(key) {
                    args.extend([
                        "-c".into(),
                        format!("{config}={}", serde_json::to_string(v)?),
                    ]);
                }
            }
            args.push("exec".into());
            if let Some(id) = session {
                args.extend(["resume".into(), id.into()]);
            }
            args.extend(["--json".into(), "--skip-git-repo-check".into()]);
            if let Some(m) = get("MODEL") {
                args.extend(["--model".into(), m.clone()]);
            }
            args.push("-".into());
        }
        "opencode" => {
            args.extend(["run".into(), "--format".into(), "json".into()]);
            if let Some(id) = session {
                args.extend(["--session".into(), id.into()]);
            }
            if let Some(m) = get("MODEL") {
                args.extend(["--model".into(), m.clone()]);
            }
            if let Some(e) = get("EFFORT") {
                args.extend(["--variant".into(), e.clone()]);
            }
            if get("PERMISSION_MODE").is_some_and(|p| p == "bypassPermissions") {
                args.push("--auto".into());
            }
        }
        _ => anyhow::bail!("unsupported native runtime {runtime}"),
    }
    Ok(args)
}

pub(super) fn parse(runtime: &str, stdout: &str) -> Result<TurnResult> {
    let mut response = TurnResult {
        text: String::new(),
        session: None,
    };
    let mut completed = false;
    let values: Vec<serde_json::Value> = if let Ok(v) = serde_json::from_str(stdout) {
        vec![v]
    } else {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    };
    for value in values {
        if value["is_error"].as_bool() == Some(true) {
            anyhow::bail!(
                "provider turn failed: {}",
                value["result"].as_str().unwrap_or("unknown error")
            );
        }
        match runtime {
            "claude-code" => {
                if let Some(t) = value["result"].as_str() {
                    response.text = t.into();
                    completed = true;
                }
                if let Some(id) = value["session_id"].as_str() {
                    response.session = Some(id.into());
                }
            }
            "codex" => {
                if value["type"] == "thread.started" {
                    response.session = value["thread_id"].as_str().map(str::to_owned);
                }
                if value["type"] == "item.completed" && value["item"]["type"] == "agent_message" {
                    if let Some(t) = value["item"]["text"].as_str() {
                        response.text = t.into();
                    }
                }
                if value["type"] == "turn.completed" {
                    completed = true;
                }
                if value["type"] == "turn.failed" || value["type"] == "error" {
                    anyhow::bail!(
                        "Codex turn failed: {}",
                        value["error"]["message"]
                            .as_str()
                            .or_else(|| value["message"].as_str())
                            .unwrap_or("provider reported an error")
                    );
                }
            }
            "opencode" => {
                if let Some(id) = value["sessionID"].as_str() {
                    response.session = Some(id.into());
                }
                if value["type"] == "text" {
                    if let Some(t) = value["part"]["text"].as_str() {
                        response.text.push_str(t);
                    }
                }
                if value["type"] == "step_finish" {
                    completed = true;
                }
                if value["type"] == "error" {
                    anyhow::bail!(
                        "OpenCode turn failed: {}",
                        value["error"]["data"]["message"]
                            .as_str()
                            .or_else(|| value["error"]["message"].as_str())
                            .unwrap_or("provider reported an error")
                    );
                }
            }
            _ => {}
        }
    }
    anyhow::ensure!(
        completed || !response.text.trim().is_empty(),
        "provider completed without a final response event"
    );
    Ok(response)
}

fn prepare_auth(env: &BTreeMap<String, String>) -> Result<()> {
    #[cfg(unix)]
    if let Some(home) = env.get("CODEX_HOME") {
        std::fs::create_dir_all(home)?;
        if let Some(operator) = std::env::var_os("HOME") {
            let source = PathBuf::from(operator).join(".codex/auth.json");
            let dest = Path::new(home).join("auth.json");
            if source.is_file() && std::fs::symlink_metadata(&dest).is_err() {
                std::os::unix::fs::symlink(source, dest)?;
            }
        }
    }
    Ok(())
}

pub(super) fn run_turn(
    compose: &Compose,
    h: AgentHandle<'_>,
    execution: &Execution,
    session: Option<&str>,
    prompt: &str,
) -> Result<TurnResult> {
    let mut env = env_file(compose, h)?;
    // Built-in aliases age independently of provider entitlements. Let native
    // CLIs choose their default unless an agent or custom descriptor pins it.
    if h.spec.model.is_none()
        && !compose
            .root
            .join("runtimes")
            .join(format!("{}.yaml", h.spec.runtime))
            .is_file()
    {
        env.remove("MODEL");
    }
    env.insert("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into());
    env.insert("OPENCODE_DISABLE_TERMINAL_TITLE".into(), "1".into());
    if execution.sandbox.r#type == SandboxType::Docker {
        let home = compose
            .root
            .join("state/container-home")
            .join(format!("{}-{}", h.project, h.agent));
        std::fs::create_dir_all(&home)?;
        env.insert("HOME".into(), home.display().to_string());
        env.insert(
            "XDG_DATA_HOME".into(),
            home.join(".local/share").display().to_string(),
        );
        env.insert(
            "XDG_CONFIG_HOME".into(),
            home.join(".config").display().to_string(),
        );
        env.insert(
            "XDG_CACHE_HOME".into(),
            home.join(".cache").display().to_string(),
        );
    }
    if execution.sandbox.r#type == SandboxType::Host {
        prepare_auth(&env)?;
    }
    let runtimes = team_core::runtimes::load_all(&compose.root)?;
    let descriptor = runtimes
        .get(&h.spec.runtime)
        .context("runtime descriptor")?;
    let mut args = descriptor.args.clone();
    args.extend(provider_args(&h.spec.runtime, &env, session)?);
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .context("agent project")?;
    let cwd = std::fs::canonicalize(compose.root.join(&project.project.cwd))?;
    let mut command = if execution.sandbox.r#type == SandboxType::Docker {
        let mut command = Command::new("docker");
        command.args(sandbox::docker_args(
            compose,
            h,
            execution,
            &env,
            &descriptor.binary,
            &args,
        )?);
        command
    } else {
        let mut command = Command::new(&descriptor.binary);
        command.args(&args).envs(&env);
        command
    };
    let output_dir = compose.root.join("state/runtime-output");
    std::fs::create_dir_all(&output_dir)?;
    let stdout_path = output_dir.join(format!("{}-{}.jsonl", h.project, h.agent));
    let stderr_path = stdout_path.with_extension("stderr");
    let stdout = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&stderr_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        stdout.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        stderr.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    command
        .current_dir(cwd)
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid is async-signal-safe and performs no allocation.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("launch {}", descriptor.binary))?;
    let write = child
        .stdin
        .take()
        .context("provider stdin")?
        .write_all(prompt.as_bytes());
    if let Err(e) = write {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.into());
    }
    let deadline = Instant::now() + Duration::from_secs(execution.turn_timeout_seconds);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if STOP.load(Ordering::Relaxed) || Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            if execution.sandbox.r#type == SandboxType::Docker {
                sandbox::stop_container(compose, h);
            }
            anyhow::bail!(
                "provider turn {}",
                if STOP.load(Ordering::Relaxed) {
                    "cancelled"
                } else {
                    "timed out"
                }
            );
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let read = |path: &Path| -> Result<String> {
        let mut body = String::new();
        File::open(path)?
            .take(16 * 1024 * 1024)
            .read_to_string(&mut body)?;
        Ok(body)
    };
    let stdout = read(&stdout_path)?;
    let parsed = parse(&h.spec.runtime, &stdout);
    if !status.success() {
        let stderr = read(&stderr_path)?;
        if let Err(error) = parsed {
            anyhow::bail!(
                "{} exited {status}: {error:#}; {}",
                h.spec.runtime,
                stderr.chars().take(2000).collect::<String>()
            );
        }
        anyhow::bail!(
            "{} exited {status}: {}",
            h.spec.runtime,
            stderr.chars().take(4000).collect::<String>()
        );
    }
    parsed
}
