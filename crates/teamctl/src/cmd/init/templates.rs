use super::*;

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
pub(super) fn collect_dir(dir: &'static Dir<'static>, out: &mut Vec<(String, &'static str)>) {
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
static PERSONAL_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/templates/personal");

/// `_common/` files shared across templates, embedded once so they stay
/// single-source. Referenced from each template's `shared` overlay under
/// the destination path that template expects.
pub(super) const COMMON_GITIGNORE: &str =
    include_str!("../../../assets/templates/_common/.gitignore");
pub(super) const COMMON_ENV: &str = include_str!("../../../assets/templates/_common/.env.example");

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
        key: "personal",
        label: "Personal agent",
        blurb: "One agent, a durable notebook, and a private web workspace.",
        dir: &PERSONAL_DIR,
        shared: &[],
    },
    Template {
        key: "ideate-and-build",
        label: "Ideate & Build",
        blurb: "An Executor, a Compass ideation partner, and two engineers — think it through, then build it.",
        dir: &IDEATE_DIR,
        // Own folder ships roles/, subagents/, charter.md, .env.example,
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
pub(super) enum Choice {
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
pub(super) fn picker_menu() -> Vec<Choice> {
    let by_key = |k: &str| {
        TEMPLATES
            .iter()
            .find(|t| t.key == k)
            .unwrap_or_else(|| panic!("missing template `{k}` in TEMPLATES"))
    };
    vec![
        Choice::Template(by_key("personal")),
        Choice::Guided,
        Choice::Template(by_key("blank")),
    ]
}

pub(super) fn template_keys() -> String {
    // `guided` ships no files but is a valid --template value; list it
    // alongside the file-shipping templates so the error message
    // matches what the CLI actually accepts.
    let mut keys: Vec<&str> = vec![GUIDED_KEY];
    keys.extend(TEMPLATES.iter().map(|t| t.key));
    keys.join(", ")
}

#[allow(dead_code)]
pub fn template_list_for_cli() -> Vec<&'static str> {
    let mut keys: Vec<&str> = vec![GUIDED_KEY];
    keys.extend(TEMPLATES.iter().map(|t| t.key));
    keys
}
