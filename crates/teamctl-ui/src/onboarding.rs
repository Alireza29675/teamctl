//! Onboarding tutorial — multi-step walkthrough of the TUI.
//!
//! Triggered automatically on first launch (sentinel file at
//! `.team/state/ui-tutorial-completed` — separate from PR-UI-1's
//! `~/.config/teamctl/ui-tutorial-completed`, which marks
//! per-machine completion; the per-team sentinel lets a brand-new
//! checkout teach a returning operator about its specific shape
//! without re-prompting machine-wide).
//!
//! Reopenable from any non-modal state via the `t` chord — the
//! statusline's always-visible `· t tutorial` hint is the
//! discovery surface. Skippable via `Esc` or first key per SPEC.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub struct Step {
    pub heading: &'static str,
    pub body: &'static str,
}

pub const STEPS: &[Step] = &[
    Step {
        heading: "Welcome to teamctl-ui",
        body: "A live view of your team. Roster on the left, focused agent in the middle, mailbox on the right. Press any key to advance, Esc to leave.",
    },
    Step {
        heading: "Roster + state glyphs",
        body: "Each agent shows a single-cell glyph: ● running · ✉ unread · ! approval pending · ✕ stopped · ? unknown. Tab to focus the roster, j/k to walk it.",
    },
    Step {
        heading: "Detail pane",
        body: "The selected agent's tmux session streams here. The title line shows which agent you're following; lines tail-clip to fit.",
    },
    Step {
        heading: "Mailbox tabs",
        body: "Inbox / Channel / Wire — `]` walks forward, `[` walks back, when the mailbox pane is focused. Tab itself always cycles pane focus, never tabs. Inbox is DMs to the focused agent; Wire is project-wide broadcasts.",
    },
    Step {
        heading: "Approvals",
        body: "When an agent files request_approval, a stripe appears at the top. Press `a` to open the modal, then `y` to approve or Shift-`N` to deny. j/k cycle if multiple are pending.",
    },
    Step {
        heading: "Compose",
        body: "@ DMs the focused agent; ! broadcasts to a channel (picker first). The editor is vim-style — i to insert, Esc to normal, Ctrl+Enter to send, Esc Esc to cancel.",
    },
    Step {
        heading: "Layouts",
        body: "Ctrl+W toggles Wall view (4 agents at once + scroll). Ctrl+M toggles Mailbox-first (channel-feed centric). Both fall back to Triptych on toggle.",
    },
    Step {
        heading: "Splits",
        body: "Ctrl+| / Ctrl+- split the detail pane so you can watch two agents at once. Ctrl+H/J/K/L cycles between splits, Ctrl+W q closes the focused one.",
    },
    Step {
        heading: "Help + quit",
        body: "? opens the full keymap. q quits (with confirm). t reopens this tour. You're ready.",
    },
];

/// Per-team sentinel file path. Returns `None` when no team root
/// is reachable — onboarding is then a noop and the tutorial
/// auto-trigger doesn't fire.
pub fn sentinel_path(team_root: &std::path::Path) -> PathBuf {
    team_root.join("state/ui-tutorial-completed")
}

pub fn has_completed(team_root: &std::path::Path) -> bool {
    sentinel_path(team_root).exists()
}

/// Mark this team's tutorial as completed by creating the
/// sentinel file. The design intent is **presence-based**: only
/// the file's existence matters, never its contents — a partial
/// write that leaves an empty / truncated file still satisfies
/// `has_completed`. That's accidentally robust to crash-during-
/// write (the auto-trigger correctly fires once, then any later
/// completion makes it stop firing forever) but the property is
/// load-bearing, not coincidental: `has_completed` deliberately
/// does NOT validate file content. Future readers tempted to
/// add atomic-rename or content-validation should know that the
/// existing crash-safety story already lives entirely in the
/// presence check; tightening write semantics doesn't strengthen
/// the contract, it just adds surface area.
pub fn mark_completed(team_root: &std::path::Path) -> std::io::Result<()> {
    let path = sentinel_path(team_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, b"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn step_count_under_ten() {
        // SPEC budget: <90s skim. 9 short steps fits the budget;
        // landmark this so future drift isn't silent.
        assert!(
            STEPS.len() <= 10,
            "tutorial bloated to {} steps",
            STEPS.len()
        );
    }

    #[test]
    fn sentinel_round_trip_in_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!has_completed(root));
        mark_completed(root).unwrap();
        assert!(has_completed(root));
        // Marker file is empty — content doesn't matter, only
        // existence does.
        let marker = sentinel_path(root);
        let bytes = fs::read(&marker).unwrap();
        assert!(bytes.is_empty());
    }
}
