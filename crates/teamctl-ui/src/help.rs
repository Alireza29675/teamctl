//! `?` help overlay — keymap registry + grouped binding list.
//!
//! The registry is the single source of truth for "what chords
//! this UI accepts." Both the help-overlay renderer and the
//! statusline's contextual hints read from this slice; the event
//! loop in `app.rs` references the same chord constants so the
//! help text never lies about what's wired up.

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub chord: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct BindingGroup {
    pub title: &'static str,
    pub bindings: &'static [Binding],
}

pub const NAVIGATION: &[Binding] = &[
    Binding {
        chord: "Tab",
        description: "cycle pane focus forward",
    },
    Binding {
        chord: "Shift+Tab",
        description: "cycle pane focus backward",
    },
    Binding {
        chord: "j / k / ↓ / ↑",
        description: "navigate within focused pane",
    },
    Binding {
        chord: "← / →",
        description: "walk mailbox tabs (when mailbox focused)",
    },
    Binding {
        chord: "Enter",
        description: "open / drill in",
    },
];

pub const LAYOUTS: &[Binding] = &[
    Binding {
        chord: "Ctrl+W",
        description: "toggle Wall layout",
    },
    Binding {
        chord: "Ctrl+M",
        description: "toggle Mailbox-first layout",
    },
    Binding {
        chord: "Ctrl+|",
        description: "split detail pane vertically",
    },
    Binding {
        chord: "Ctrl+-",
        description: "split detail pane horizontally",
    },
    Binding {
        chord: "Ctrl+H/J/K/L",
        description: "vim window-motion across splits",
    },
    Binding {
        chord: "Ctrl+W q / Ctrl+Q",
        description: "close focused split",
    },
];

pub const COMPOSE: &[Binding] = &[
    Binding {
        chord: "@",
        description: "DM the focused agent",
    },
    Binding {
        chord: "!",
        description: "broadcast to a channel (picker)",
    },
    Binding {
        chord: "Esc Enter",
        description: "send the composed message (terminal-universal)",
    },
    Binding {
        chord: "Esc Esc",
        description: "cancel compose",
    },
    Binding {
        chord: ":wq / :q",
        description: "ex-command send / cancel",
    },
    Binding {
        chord: "i / a / o",
        description: "enter insert mode",
    },
    Binding {
        chord: "w / b / e",
        description: "word motions in normal mode",
    },
    Binding {
        chord: "dd / yy / p",
        description: "line ops in normal mode",
    },
];

pub const STREAM_KEYS: &[Binding] = &[
    Binding {
        chord: "Ctrl+E",
        description: "stream keys to focused agent's tmux pane (when detail focused)",
    },
    Binding {
        chord: "Esc",
        description: "exit stream-keys mode",
    },
];

pub const APPROVALS: &[Binding] = &[
    Binding {
        chord: "a",
        description: "open approvals modal (when pending)",
    },
    Binding {
        chord: "y",
        description: "approve focused",
    },
    Binding {
        chord: "Shift-N",
        description: "deny focused (Shift-gated)",
    },
    Binding {
        chord: "j / k",
        description: "cycle through pending approvals",
    },
];

// T-131: per-row mailbox UX — row scroll (PR-1), filter+search
// (PR-2), detail modal (PR-3), time indicator (PR-4). All gated on
// `Pane::Mailbox` focused.
pub const MAILBOX: &[Binding] = &[
    Binding {
        chord: "j / k / ↓ / ↑",
        description: "row down / up (when mailbox focused)",
    },
    Binding {
        chord: "PageDown / PageUp",
        description: "jump a screen down / up",
    },
    Binding {
        chord: "Home / End",
        description: "jump to first / last row",
    },
    Binding {
        chord: "← / →",
        description: "cycle mailbox tabs",
    },
    Binding {
        chord: "f",
        description: "filter rows by sender substring (Esc reverts, Enter keeps)",
    },
    Binding {
        chord: "/",
        description: "search rows by body substring (Esc reverts, Enter keeps)",
    },
    Binding {
        chord: "Enter",
        description: "open detail modal on the selected row",
    },
    Binding {
        chord: "Esc / q (in modal)",
        description: "close the detail modal",
    },
];

pub const SYSTEM: &[Binding] = &[
    Binding {
        chord: "?",
        description: "this help overlay",
    },
    Binding {
        chord: "t",
        description: "open / reopen tutorial",
    },
    Binding {
        chord: "q",
        description: "quit (with confirm)",
    },
    Binding {
        chord: "Esc",
        description: "close modal / cancel",
    },
];

pub const ALL_GROUPS: &[BindingGroup] = &[
    BindingGroup {
        title: "Navigation",
        bindings: NAVIGATION,
    },
    BindingGroup {
        title: "Layouts",
        bindings: LAYOUTS,
    },
    BindingGroup {
        title: "Compose",
        bindings: COMPOSE,
    },
    BindingGroup {
        title: "Stream keys",
        bindings: STREAM_KEYS,
    },
    BindingGroup {
        title: "Mailbox",
        bindings: MAILBOX,
    },
    BindingGroup {
        title: "Approvals",
        bindings: APPROVALS,
    },
    BindingGroup {
        title: "System",
        bindings: SYSTEM,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_seven_groups() {
        // T-108 added Stream-keys between Compose and Approvals;
        // T-131 PR-4 added Mailbox between Stream-keys and
        // Approvals. Update this count when groups change.
        assert_eq!(ALL_GROUPS.len(), 7);
    }

    #[test]
    fn registry_covers_central_chords() {
        let bindings: Vec<&str> = ALL_GROUPS
            .iter()
            .flat_map(|g| g.bindings.iter().map(|b| b.chord))
            .collect();
        for must_have in [
            "Tab",
            "Ctrl+W",
            "Ctrl+E",
            "@",
            "!",
            "a",
            "y",
            "Shift-N",
            "?",
            "t",
            "q",
            "Esc Enter",
            // T-131 PR-4: mailbox UX chords surfaced in the
            // registry so the help overlay never lies about what's
            // wired up.
            "f",
            "/",
            "PageDown / PageUp",
            "Home / End",
            "Esc / q (in modal)",
        ] {
            assert!(
                bindings.contains(&must_have),
                "registry missing chord {must_have}"
            );
        }
    }
}
