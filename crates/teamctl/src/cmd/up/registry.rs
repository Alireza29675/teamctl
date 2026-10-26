use super::*;

/// Build the registry rows for the in-scope projects: one
/// [`team_core::registry::TeamEntry`] per project, agents folded in and
/// sorted. `scoped` (a `--project` filter) restricts the set to that
/// project; `None` records every project in the compose. The row's
/// `agents` is the project's full declared roster — a `--agent` filter
/// narrows what `up` *spawns*, not the team membership the registry
/// records (which is the shape `ps` wants to show). Pure over its inputs
/// so the mapping is unit-testable without a live `Compose` or `$HOME`.
pub(super) fn registry_entries<'a>(
    agents: impl Iterator<Item = (&'a str, &'a str)>,
    root: &Path,
    tmux_prefix: &str,
    scoped: Option<&str>,
    started_at: &str,
) -> Vec<team_core::registry::TeamEntry> {
    let mut by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (project, agent) in agents {
        if scoped.is_some_and(|id| id != project) {
            continue;
        }
        by_project
            .entry(project)
            .or_default()
            .push(agent.to_string());
    }
    by_project
        .into_iter()
        .map(|(project, mut names)| {
            names.sort();
            let sessions = names
                .iter()
                .map(|agent| format!("{tmux_prefix}{project}-{agent}").replace('.', "_"))
                .collect();
            team_core::registry::TeamEntry {
                project_id: project.to_string(),
                root: root.to_path_buf(),
                tmux_prefix: tmux_prefix.to_string(),
                agents: names,
                sessions,
                started_at: started_at.to_string(),
            }
        })
        .collect()
}

/// T-469: refuse a launch that would alias an already-running team. teamctl
/// identity is name-keyed (session UUID, tmux name, mailbox key), never
/// path-keyed, so two teams with the same `project_id` in different folders
/// alias each other's sessions — the crash-loop behind the owner's report
/// (tg 2318). If the durable registry already records any in-scope
/// `project_id` at a DIFFERENT, still-live root, bail before any side
/// effects, naming the conflicting directory. A re-up of the same
/// `(project_id, root)` is fine.
///
/// `pub(super)` so `reload` — the other spawn path — shares the guard.
/// A missing registry is empty; read errors propagate rather than treating
/// an inaccessible ownership store as an available session namespace.
/// The check iterates per declared agent, so a project with zero agents is
/// not examined — it has no session/tmux/mailbox identity to alias.
pub(crate) fn guard_no_name_collision(compose: &Compose, scoped: Option<&str>) -> Result<()> {
    let Some(dir) = team_core::registry::config_dir() else {
        return Ok(());
    };
    let reg = team_core::registry::load(&dir)?;
    guard_no_name_collision_in(&reg, compose, scoped, &|p| p.exists())
}

/// Inner half of [`guard_no_name_collision`] with the registry and the
/// liveness check injected, so the guard's decision is unit-testable without
/// `$HOME` or the real filesystem.
pub(super) fn guard_no_name_collision_in(
    reg: &team_core::registry::Registry,
    compose: &Compose,
    scoped: Option<&str>,
    path_exists: &impl Fn(&Path) -> bool,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for h in compose.agents() {
        if scoped.is_some_and(|s| s != h.project) || !seen.insert(h.project) {
            continue;
        }
        if let Some(other) =
            team_core::registry::same_name_other_root(reg, h.project, &compose.root, path_exists)
        {
            bail!(
                "project `{}` is already up at {} — two teams with the same project id in \
                 different folders alias each other (they share session ids, tmux names, and \
                 mailbox keys) and can crash-loop. Rename the project here, or `teamctl down` \
                 the other team first.",
                h.project,
                other.display()
            );
        }
    }
    let proposed = session_names(compose, scoped)?;
    for team in &reg.teams {
        if team.root == compose.root || team_core::registry::is_orphan(team, path_exists) {
            continue;
        }
        let mut reserved: BTreeSet<_> = team.sessions.iter().map(|s| s.replace('.', "_")).collect();
        reserved.extend(team.agents.iter().map(|agent| {
            format!("{}{}-{agent}", team.tmux_prefix, team.project_id).replace('.', "_")
        }));
        if let Some(snapshot) = super::super::snapshot::read(&team.root) {
            reserved.extend(
                snapshot
                    .agents
                    .into_iter()
                    .filter(|(id, _)| {
                        id.split_once(':')
                            .is_some_and(|(project, _)| project == team.project_id)
                    })
                    .map(|(_, entry)| entry.tmux_session.replace('.', "_")),
            );
        }
        reserved.extend(
            super::super::bot::persisted_sessions(&team.root)?
                .into_iter()
                .filter(|(id, _)| {
                    id.split_once(':')
                        .is_some_and(|(project, _)| project == team.project_id)
                })
                .map(|(_, session)| session.replace('.', "_")),
        );
        reserved.extend(
            super::super::adapters::lifecycle::load(&team.root)?
                .into_values()
                .filter(|row| {
                    row.agent
                        .split_once(':')
                        .is_some_and(|(project, _)| project == team.project_id)
                })
                .map(|row| row.session.replace('.', "_")),
        );
        // Legacy registry rows predate explicit bot reservations.
        if let Ok(other) = Compose::load_read_only(&team.root) {
            reserved.extend(super::super::adapters::lifecycle::session_names(
                &other,
                Some(&team.project_id),
            )?);
            reserved.extend(
                super::super::bot::bot_specs(&other)
                    .into_iter()
                    .filter(|spec| {
                        spec.manager
                            .split_once(':')
                            .is_some_and(|(project, _)| project == team.project_id)
                    })
                    .map(|spec| spec.session.replace('.', "_")),
            );
        }
        if let Some(session) = proposed.intersection(&reserved).next() {
            bail!("tmux session `{session}` is reserved by project `{}` at {}; choose distinct project/agent names or bring that team down first", team.project_id, team.root.display());
        }
    }
    Ok(())
}

fn session_names(compose: &Compose, scoped: Option<&str>) -> Result<BTreeSet<String>> {
    let in_scope = |id: &str| {
        scoped.is_none_or(|scope| {
            id.split_once(':')
                .is_some_and(|(project, _)| project == scope)
        })
    };
    let mut names: BTreeSet<_> = compose
        .agents()
        .filter(|h| scoped.is_none_or(|scope| h.project == scope))
        .map(|h| {
            format!(
                "{}{}-{}",
                compose.global.supervisor.tmux_prefix, h.project, h.agent
            )
            .replace('.', "_")
        })
        .collect();
    names.extend(
        super::super::bot::bot_specs(compose)
            .into_iter()
            .filter(|s| in_scope(&s.manager))
            .map(|s| s.session.replace('.', "_")),
    );
    if let Some(snapshot) = super::super::snapshot::read(&compose.root) {
        names.extend(
            snapshot
                .agents
                .into_iter()
                .filter(|(id, _)| in_scope(id))
                .map(|(_, entry)| entry.tmux_session.replace('.', "_")),
        );
    }
    names.extend(
        super::super::bot::persisted_sessions(&compose.root)?
            .into_iter()
            .filter(|(id, _)| in_scope(id))
            .map(|(_, name)| name.replace('.', "_")),
    );
    names.extend(super::super::adapters::lifecycle::session_names(
        compose, scoped,
    )?);
    Ok(names)
}

fn complete_entries(
    compose: &Compose,
    scoped: Option<&str>,
) -> Result<Vec<team_core::registry::TeamEntry>> {
    let mut entries = registry_entries(
        compose.agents().map(|h| (h.project, h.agent)),
        &compose.root,
        &compose.global.supervisor.tmux_prefix,
        scoped,
        &team_core::registry::now_rfc3339(),
    );
    for entry in &mut entries {
        entry.sessions = session_names(compose, Some(&entry.project_id))?
            .into_iter()
            .collect();
    }
    Ok(entries)
}

/// Persist this team's registry rows. Resolves the config dir from `$HOME`
/// (skipping with a warning if unset), then delegates to
/// [`record_in_registry_in`]. Never returns an error: the registry is a
/// convenience side store, and a failure here must not undo a successful
/// `up`. `pub(super)` so `reload` can refresh the registry after applying
/// a diff (T-468) — keeping `ps` / reaping accurate across reloads.
pub(crate) fn record_in_registry(compose: &Compose, scoped: Option<&str>) {
    let Some(dir) = team_core::registry::config_dir() else {
        eprintln!("warn · teams registry: neither HOME nor USERPROFILE set, skipping");
        return;
    };
    record_in_registry_in(&dir, compose, scoped);
}

/// Reserve ownership before spawning. If a later agent fails to launch,
/// its successfully started siblings still belong to this root.
pub(crate) fn reserve_in_registry(compose: &Compose, scoped: Option<&str>) -> Result<()> {
    let Some(dir) = team_core::registry::config_dir() else {
        return Ok(());
    };
    let entries = complete_entries(compose, scoped)?;
    team_core::registry::upsert_many(&dir, entries)
}

/// Inner half of [`record_in_registry`] with the config dir injected, so
/// the `up` → `teams.json` write path is testable against a real `Compose`
/// without resolving (or mutating) `$HOME`.
pub(super) fn record_in_registry_in(dir: &Path, compose: &Compose, scoped: Option<&str>) {
    if let Err(e) = complete_entries(compose, scoped)
        .and_then(|entries| team_core::registry::upsert_many(dir, entries))
    {
        eprintln!("warn · teams registry: {e:#}");
    }
}
