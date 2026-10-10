use super::*;

/// Fully loaded compose tree: global + resolved projects.
#[derive(Debug, Clone)]
pub struct Compose {
    pub root: PathBuf,
    pub global: Global,
    pub projects: Vec<Project>,
}

impl Compose {
    /// Walk up from `start` looking for the **first** `.team/team-compose.yaml`
    /// and return the directory containing the compose file (the "root"),
    /// suitable for passing to [`Compose::load`]. The first hit wins; we do
    /// not keep walking past it to look for a parent `.team/`.
    ///
    /// This is the equivalent of git's `.git/` discovery — once a repo carries
    /// a `.team/` folder, every `teamctl` subcommand finds it from anywhere
    /// inside the tree. T-008 retired the legacy flat-layout fallback and
    /// the second-hit / parent-`.team/` walk: the convention is `.team/` and
    /// the nearest one wins, no exceptions.
    pub fn discover(start: &Path) -> anyhow::Result<PathBuf> {
        let start = start
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("canonicalize {}: {e}", start.display()))?;
        let mut cur: Option<&Path> = Some(&start);
        while let Some(dir) = cur {
            let candidate = dir.join(".team").join("team-compose.yaml");
            if candidate.is_file() {
                return Ok(dir.join(".team"));
            }
            cur = dir.parent();
        }
        Err(anyhow::anyhow!(
            "no `.team/team-compose.yaml` found in {} or any parent",
            start.display()
        ))
    }

    /// Parse `team-compose.yaml` at `root` and every referenced project file.
    pub fn load(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::load_internal(root.as_ref(), true)
    }

    /// Parse and normalize the compose in memory without rewriting legacy
    /// manifests. Use for previews and discovery of another team's state.
    pub fn load_read_only(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::load_internal(root.as_ref(), false)
    }

    fn load_internal(root: &Path, rewrite_legacy: bool) -> anyhow::Result<Self> {
        let root = root.to_path_buf();
        let global_path = root.join("team-compose.yaml");
        let raw = std::fs::read_to_string(&global_path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", global_path.display()))?;
        let global: Global = serde_yaml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", global_path.display()))?;

        // T-265 PR-a: legacy-`2` auto-rewrite on load. When the
        // operator's compose still uses the pre-semver shape
        // (`version: 2`, integer literal), the Deserialize impl on
        // `SchemaVersion` has already coerced the in-memory value to
        // `"2.0.0"` and flagged `from_legacy_int = true`. Now we
        // best-effort rewrite the file so the on-disk shape matches
        // the runtime semantics — eliminating the file-vs-runtime
        // divergence the operator would otherwise see in git diff
        // forever. On RO filesystems (CI sandboxes, immutable image
        // mounts) or any other write failure, we emit a single warn
        // and proceed with the in-memory normalized value rather
        // than hard-erroring — owner-ratified (tg 3440, "RO-FS
        // degrades to in-memory + warning"). Single hardcoded
        // legacy-value exception, NOT the general migration engine
        // — that stays deferred to its own ticket per #265's
        // non-goals.
        if rewrite_legacy && global.version.from_legacy_int {
            if let Err(e) = rewrite_legacy_version_in_file(&global_path, &raw) {
                tracing::warn!(
                    target: "team-core::compose",
                    "could not rewrite legacy `version: 2` in {}: {e}; \
                     proceeding with in-memory `\"2.0.0\"`",
                    global_path.display()
                );
            }
        }

        let mut projects = Vec::with_capacity(global.projects.len());
        for r in &global.projects {
            let p = root.join(&r.file);
            let parsed: Project = serde_yaml::from_str(
                &std::fs::read_to_string(&p)
                    .map_err(|e| anyhow::anyhow!("read {}: {e}", p.display()))?,
            )
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", p.display()))?;
            projects.push(parsed);
        }

        Ok(Self {
            root,
            global,
            projects,
        })
    }

    /// Return every agent in the compose tree tagged with manager/worker.
    pub fn agents(&self) -> impl Iterator<Item = AgentHandle<'_>> {
        self.projects.iter().flat_map(|p| {
            p.managers
                .iter()
                .map(move |(id, a)| AgentHandle {
                    project: &p.project.id,
                    agent: id,
                    spec: a,
                    is_manager: true,
                })
                .chain(p.workers.iter().map(move |(id, a)| AgentHandle {
                    project: &p.project.id,
                    agent: id,
                    spec: a,
                    is_manager: false,
                }))
        })
    }
}

/// T-265 PR-a: rewrite the legacy `version: 2` integer literal in
/// `team-compose.yaml` to the semver string form `"2.0.0"`,
/// preserving comments + key ordering via the `yaml_edit` substrate.
/// Caller has already loaded the raw text (passed as `raw` to avoid
/// a second disk read) and decided we're in the legacy path.
fn rewrite_legacy_version_in_file(path: &Path, raw: &str) -> anyhow::Result<()> {
    let updated = crate::yaml_edit::set_top_level_scalar(raw, "version", "\"2.0.0\"")?;
    std::fs::write(path, updated).map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct AgentHandle<'a> {
    pub project: &'a str,
    pub agent: &'a str,
    pub spec: &'a Agent,
    pub is_manager: bool,
}

impl AgentHandle<'_> {
    /// Canonical id as `<project>:<agent>`.
    pub fn id(&self) -> String {
        format!("{}:{}", self.project, self.agent)
    }
}
