use super::*;
use std::process::Command;

pub(super) fn network_name(compose: &Compose) -> String {
    format!(
        "teamctl-{}",
        &blake3::hash(compose.root.to_string_lossy().as_bytes()).to_hex()[..16]
    )
}

pub(super) fn validate(compose: &Compose, h: AgentHandle<'_>, execution: &Execution) -> Result<()> {
    let sandbox = &execution.sandbox;
    anyhow::ensure!(
        sandbox
            .image
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty() && !v.starts_with('-')),
        "{}: Docker sandbox requires an image",
        h.id()
    );
    for name in &sandbox.env {
        anyhow::ensure!(valid_env_name(name), "invalid sandbox env reference {name}");
    }
    for mount in &sandbox.mounts {
        anyhow::ensure!(
            mount.target.is_absolute(),
            "Docker mount target must be absolute: {}",
            mount.target.display()
        );
        let source = compose.root.join(&mount.source);
        anyhow::ensure!(
            source.exists(),
            "Docker mount source does not exist: {}",
            source.display()
        );
        anyhow::ensure!(
            !source.to_string_lossy().contains("docker.sock")
                && !mount.target.to_string_lossy().contains("docker.sock"),
            "Docker daemon socket mounts are not supported"
        );
        // Docker's --mount CSV syntax must remain a single unambiguous mount.
        anyhow::ensure!(
            !source.to_string_lossy().contains(',')
                && !mount.target.to_string_lossy().contains(','),
            "Docker mount paths cannot contain commas"
        );
    }
    Ok(())
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn prepare(compose: &Compose, h: AgentHandle<'_>, execution: &Execution) -> Result<()> {
    if execution.sandbox.r#type != SandboxType::Docker {
        return Ok(());
    }
    validate(compose, h, execution)?;
    let network = network_name(compose);
    if !Command::new("docker")
        .args(["network", "inspect", &network])
        .output()?
        .status
        .success()
    {
        let output = Command::new("docker")
            .args([
                "network",
                "create",
                "--driver",
                "bridge",
                "--label",
                "teamctl.managed=true",
                &network,
            ])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "create Docker team network: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Mount the workspace/compose tree at identical absolute paths, preserving
/// generated MCP configs and paths shared by all collaborating agents.
pub(super) fn docker_args(
    compose: &Compose,
    h: AgentHandle<'_>,
    execution: &Execution,
    env: &BTreeMap<String, String>,
    program: &str,
    args: &[String],
) -> Result<Vec<String>> {
    validate(compose, h, execution)?;
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .context("project")?;
    let workspace = std::fs::canonicalize(compose.root.join(&project.project.cwd))?;
    let root = std::fs::canonicalize(&compose.root)?;
    let mut docker = vec![
        "run".into(),
        "--rm".into(),
        "--init".into(),
        "--interactive".into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
        "--network".into(),
        network_name(compose),
        "--name".into(),
        format!("{}-{}-{}", network_name(compose), h.project, h.agent),
        "--label".into(),
        "teamctl.managed=true".into(),
    ];
    #[cfg(unix)]
    {
        docker.extend([
            "--user".into(),
            format!("{}:{}", unsafe { libc::getuid() }, unsafe {
                libc::getgid()
            }),
        ]);
    }
    let mut paths = vec![&root];
    if workspace != root {
        paths.push(&workspace);
    }
    for path in paths {
        anyhow::ensure!(
            !path.to_string_lossy().contains(','),
            "Docker workspace paths cannot contain commas"
        );
        docker.extend([
            "--mount".into(),
            format!(
                "type=bind,source={},target={}",
                path.display(),
                path.display()
            ),
        ]);
    }
    // The shared workspace is a collaboration boundary, but ordinary root
    // .env files and sibling agents' credential stores need not be exposed.
    // Explicit env grants/mounts remain the way to provide a credential.
    let current = format!("{}-{}", h.project, h.agent);
    let mut hidden_files = std::collections::BTreeSet::new();
    for directory in [&root, &workspace] {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (name == ".env" || name.starts_with(".env.")) && entry.path().is_file() {
                hidden_files.insert(entry.path());
            }
        }
    }
    for name in ["codex-home", "opencode-home", "container-home"] {
        let directory = root.join("state").join(name);
        if !directory.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_name() != current.as_str() && entry.path().is_dir() {
                docker.extend([
                    "--mount".into(),
                    format!(
                        "type=tmpfs,destination={},tmpfs-mode=0700",
                        entry.path().display()
                    ),
                ]);
            }
        }
    }
    let directory = root.join("state/envs");
    if directory.is_dir() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_name() != format!("{current}.env").as_str() && entry.path().is_file() {
                hidden_files.insert(entry.path());
            }
        }
    }
    for path in hidden_files {
        anyhow::ensure!(
            !path.to_string_lossy().contains(','),
            "Docker secret paths cannot contain commas"
        );
        docker.extend([
            "--mount".into(),
            format!(
                "type=bind,source=/dev/null,target={},readonly",
                path.display()
            ),
        ]);
    }
    for mount in &execution.sandbox.mounts {
        docker.extend([
            "--mount".into(),
            format!(
                "type=bind,source={},target={}{}",
                compose.root.join(&mount.source).display(),
                mount.target.display(),
                if mount.read_only { ",readonly" } else { "" }
            ),
        ]);
    }
    for (key, value) in env {
        docker.extend(["--env".into(), format!("{key}={value}")]);
    }
    for name in &execution.sandbox.env {
        anyhow::ensure!(
            std::env::var_os(name).is_some(),
            "sandbox env reference {name} is unset"
        );
        // Values stay in process environment, never in saved commands/config.
        docker.extend(["--env".into(), name.clone()]);
    }
    docker.extend([
        "--workdir".into(),
        execution
            .sandbox
            .workdir
            .as_ref()
            .unwrap_or(&workspace)
            .display()
            .to_string(),
        "--entrypoint".into(),
        program.into(),
        execution.sandbox.image.clone().context("Docker image")?,
    ]);
    docker.extend(args.iter().cloned());
    Ok(docker)
}

pub(super) fn stop_container(compose: &Compose, h: AgentHandle<'_>) {
    let _ = Command::new("docker")
        .args([
            "rm",
            "--force",
            &format!("{}-{}-{}", network_name(compose), h.project, h.agent),
        ])
        .output();
}
