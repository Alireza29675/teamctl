//! `notify`-based file-watch on the broker SQLite at `state/mailbox.db`.
//!
//! Replaces (or augments) the 1s long-poll established in PR-UI-2:
//! when the platform supports `notify` (Linux inotify / macOS
//! FSEvents / Windows ReadDirectoryChangesW), the run loop refreshes
//! immediately on a `mailbox.db`-WAL/SHM write event. Platforms
//! without `notify` support fall back to the 1s poll — same shape
//! as the truecolor-detection capability fallback in PR-UI-1.
//!
//! The watcher writes to a shared `AtomicBool` "dirty" flag rather
//! than emitting events through a channel, because the run loop
//! already has its own poll cadence and we just need the watcher to
//! say "something changed, refresh sooner than the deadline."

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{Event, EventKind, RecursiveMode, Watcher};

/// Spawn a `notify` watcher rooted at the broker's `state/`
/// directory. Returns `None` on platforms (or filesystems) where
/// `notify` can't bring up a recommended watcher — the run loop
/// then falls back to the 1s poll. The returned `Watch` keeps the
/// underlying watcher alive; drop it to stop watching.
pub struct Watch {
    /// Shared dirty flag — flipped to `true` on every relevant
    /// filesystem event. The run loop reads + clears it via
    /// `take_dirty`.
    pub dirty: Arc<AtomicBool>,
    /// Holds the platform watcher so it doesn't drop and stop
    /// emitting events. The field is read in
    /// [`Watch::take_dirty`]'s impl boundary only, never directly.
    _watcher: Option<Box<dyn Watcher + Send>>,
}

impl Watch {
    /// Construct a watch with no underlying watcher. Used by tests
    /// and as the fallback when `notify::recommended_watcher`
    /// fails to initialise.
    pub fn idle() -> Self {
        Self {
            dirty: Arc::new(AtomicBool::new(false)),
            _watcher: None,
        }
    }

    /// Try to build a recommended watcher rooted at `state_dir`.
    /// Returns an idle (no-watcher) Watch on any failure so the
    /// caller can still rely on the dirty-flag interface even when
    /// the platform doesn't support filesystem notifications.
    pub fn try_new(state_dir: &Path) -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let dirty_for_cb = dirty.clone();
        let cb = move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                if relevant(&ev.kind) {
                    dirty_for_cb.store(true, Ordering::SeqCst);
                }
            }
        };
        let watcher = notify::recommended_watcher(cb).ok();
        let mut watcher: Option<Box<dyn Watcher + Send>> =
            watcher.map(|w| Box::new(w) as Box<dyn Watcher + Send>);
        if let Some(w) = watcher.as_mut() {
            // `mailbox.db` lives at `<state_dir>/mailbox.db`; SQLite
            // also writes WAL + SHM siblings (`mailbox.db-wal`,
            // `mailbox.db-shm`) on every commit. Watching the
            // parent dir non-recursively catches all three with one
            // subscription.
            if w.watch(state_dir, RecursiveMode::NonRecursive).is_err() {
                // Watcher started but couldn't subscribe (permissions,
                // missing dir) — fall back to idle.
                return Self::idle();
            }
        }
        Self {
            dirty,
            _watcher: watcher,
        }
    }

    /// Read + clear the dirty flag. Returns `true` exactly once per
    /// batch of events the watcher saw since the previous call.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }
}

/// Filter `notify` event kinds down to the ones SQLite actually
/// triggers on commit. Create / Modify / Remove cover every shape
/// we care about; `Access` events (read-only opens) would refresh
/// uselessly on every UI tick and are dropped here.
fn relevant(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind};

    #[test]
    fn idle_watch_is_never_dirty() {
        let w = Watch::idle();
        assert!(!w.take_dirty());
        assert!(!w.take_dirty());
    }

    #[test]
    fn dirty_flag_clears_on_take() {
        let w = Watch::idle();
        w.dirty.store(true, Ordering::SeqCst);
        assert!(w.take_dirty());
        assert!(!w.take_dirty(), "second call sees the cleared flag");
    }

    #[test]
    fn relevant_kinds_match_sqlite_commit_shape() {
        assert!(relevant(&EventKind::Create(CreateKind::File)));
        assert!(relevant(&EventKind::Modify(ModifyKind::Any)));
        assert!(relevant(&EventKind::Remove(RemoveKind::File)));
        // Access events (e.g. an `inbox_peek` reader open) must
        // not trigger a refresh — the UI doesn't care about reads.
        assert!(!relevant(&EventKind::Access(
            notify::event::AccessKind::Open(notify::event::AccessMode::Read)
        )));
    }

    #[test]
    fn try_new_on_missing_dir_returns_idle() {
        // Non-existent path → watcher subscribe fails → fallback.
        let w = Watch::try_new(Path::new("/definitely/does/not/exist/teamctl-ui-test"));
        assert!(!w.take_dirty(), "idle fallback never goes dirty");
    }
}
