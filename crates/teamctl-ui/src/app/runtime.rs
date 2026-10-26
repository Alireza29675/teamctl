//! Runtime refresh, pane capture, sizing, and the event loop.

use super::{
    draw, handle_event, App, Stage, PANE_REFRESH_INTERVAL, POLL_INTERVAL, REFRESH_INTERVAL,
    SPLASH_AUTO_DISMISS,
};
use crate::approvals::{ApprovalSource, BrokerApprovalSource, CliApprovalDecider};
use crate::compose::CliMessageSender;
use crate::data::TeamSnapshot;
use crate::keysender::{AsyncKeySender, TmuxKeySender};
use crate::mailbox::{BrokerMailboxSource, MailboxSource, MailboxTab};
use crate::pane::{PaneSource, TmuxPaneSource};
use crate::triptych::MainLayout;
use crate::watch::Watch;
use anyhow::Result;
use crossterm::event;
use ratatui::{backend::Backend, Terminal};
use std::time::Instant;

/// Refresh the roster, pane capture, approvals, and mailbox tabs.
/// Pulled out so tests can drive a
/// single tick deterministically against `MockPaneSource` and
/// `MockMailboxSource` without going through the event loop.
pub fn refresh<P: PaneSource, M: MailboxSource, A: ApprovalSource>(
    app: &mut App,
    pane_source: &P,
    mailbox_source: &M,
    approval_source: &A,
) {
    if let Ok(Some(snapshot)) = current_snapshot(app) {
        app.replace_team(snapshot);
    }
    if let Some(session) = app
        .stream_target_session()
        .filter(|_| app.layout != MainLayout::Chat)
    {
        if let Ok(lines) = pane_source.capture(&session) {
            app.set_detail_buffer(lines);
        }
    } else {
        app.detail_buffer.clear();
    }
    refresh_mailbox(app, mailbox_source);
    refresh_approvals(app, approval_source);
    app.last_refresh = Instant::now();
    app.last_pane_refresh = Instant::now();
}

/// Approvals-only refresh. Extracted on the same shape as
/// `refresh_mailbox` — PR-UI-5+ can call it on its own cadence
/// (e.g. in response to a `notify` signal) without re-running the
/// heavier paths. Transient read failures retain the queue and selection.
pub fn refresh_approvals<A: ApprovalSource>(app: &mut App, approval_source: &A) {
    match approval_source.pending() {
        Ok(approvals) => app.replace_approvals(approvals),
        Err(error) => app.approval_error = Some(format!("refresh approvals: {error}")),
    }
}

/// Mailbox-only refresh — extracted so PR-UI-4+ can call it on its
/// own cadence (e.g. in response to a broker INSERT signal) without
/// re-running the heavier compose + tmux capture path. PR-UI-3
/// just calls it from the main `refresh` once per tick.
pub fn refresh_mailbox<M: MailboxSource>(app: &mut App, mailbox_source: &M) {
    let Some(agent_id) = app.selected_agent_id() else {
        // No agent focused → nothing to fetch. Buffers were already
        // reset on selection change so the empty-state hint shows.
        return;
    };
    let project_id = app
        .selected_agent
        .and_then(|i| app.team.agents.get(i))
        .map(|a| a.project.clone())
        .unwrap_or_default();
    // #462: record the focused agent so the Inbox merge can filter
    // channel/wire to inbound (`sender != me`). Set before the extends
    // so the Inbox cursor math sees the filtered view.
    app.mailbox.agent_id = agent_id.clone();
    // Capture Inbox tail-follow before the extends: per-tab `extend`
    // only follows the tab it touches, but channel/wire arrivals grow
    // the merged Inbox view too, so we re-anchor it once at the end.
    let inbox_was_at_tail = app.mailbox.inbox_at_tail();
    // All four data shapes are still fetched and folded into their own
    // buffers — the channel/wire buffers back both the Inbox merge and
    // the MailboxFirst channel feed (#462 changed only the tab surface,
    // not ingestion).
    if let Ok(batch) = mailbox_source.inbox(&agent_id, app.mailbox.inbox_after) {
        app.mailbox.extend(MailboxTab::Inbox, batch);
    }
    if let Ok(batch) = mailbox_source.sent(&agent_id, app.mailbox.sent_after) {
        app.mailbox.extend(MailboxTab::Sent, batch);
    }
    if let Ok(batch) = mailbox_source.channel_feed(&agent_id, app.mailbox.channel_after) {
        app.mailbox.extend(MailboxTab::Channel, batch);
    }
    if let Ok(batch) = mailbox_source.wire(&project_id, app.mailbox.wire_after) {
        app.mailbox.extend(MailboxTab::Wire, batch);
    }
    if inbox_was_at_tail {
        app.mailbox.follow_inbox_tail();
    }
}

pub fn run<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut app = App::new();
    if let Some(snapshot) = TeamSnapshot::discover_and_load()? {
        app.replace_team(snapshot);
    }
    let pane_source = TmuxPaneSource;
    let decider = CliApprovalDecider;
    let sender = CliMessageSender;
    // #386: wrap the blocking `tmux send-keys` round-trip so it runs on
    // a background thread — the render/event loop enqueues each keystroke
    // in ~µs and stays free to observe the next key and redraw, instead
    // of blocking ~3ms per key on the inline subprocess.
    let key_sender = AsyncKeySender::new(TmuxKeySender);
    let pane_resizer = crate::pane_resize::TmuxPaneResizer;
    // First refresh resolves the team root; only then can we
    // bring up the file-watcher, which keys on `<root>/state/`.
    refresh_with_default_sources(&mut app, &pane_source);
    let mut watched_dir = app
        .mailbox_db_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let mut watch = Watch::try_new(&watched_dir);
    let mut chat_worker: Option<(std::path::PathBuf, crate::chat::ChatWorker)> = None;
    while app.running {
        tick_chat(&mut app, &mut chat_worker);
        // T-131 PR-4: refresh the render-tick clock so the mailbox
        // relative-time indicator reads a fresh `now_secs` each
        // draw. Keeping this on App (not in render) makes render a
        // pure function — tests can pin time deterministically.
        app.now_secs = chrono::Utc::now().timestamp() as f64;
        terminal
            .draw(|f| draw(f, &app))
            .map_err(|error| anyhow::anyhow!("draw terminal: {error}"))?;
        // T-199: after every frame, push the focused agent's inner
        // tmux pane to match teamctl-ui's Detail rect so claude
        // reflows when the operator resizes the host terminal (or
        // when focus switches to a different agent whose pane was
        // last sized for a different layout). The cache inside
        // `sync_focused_pane_size_to` keeps this to one HashMap
        // lookup per frame in the no-op case.
        let term_sz = terminal
            .size()
            .map_err(|error| anyhow::anyhow!("read terminal size: {error}"))?;
        let term_area = ratatui::layout::Rect::new(0, 0, term_sz.width, term_sz.height);
        let chat_areas = crate::chat::geometry(crate::pane_resize::main_areas(term_area)[0]);
        app.chat_viewport = (
            chat_areas[1].width.saturating_sub(2),
            chat_areas[1].height.saturating_sub(3),
        );
        sync_focused_pane_size_to(&mut app, term_area, &pane_resizer);
        if event::poll(POLL_INTERVAL)? {
            // The mailbox source for handle_event mirrors the
            // refresh path; the same db_path key avoids divergence
            // between read + write fanout.
            let db_path = app.mailbox_db_path.clone();
            let mailbox_source = BrokerMailboxSource::new(db_path);
            handle_event(
                &mut app,
                event::read()?,
                &decider,
                &sender,
                &mailbox_source,
                &key_sender,
            );
        }
        if matches!(app.stage, Stage::Splash) && app.splash_started.elapsed() >= SPLASH_AUTO_DISMISS
        {
            app.dismiss_splash();
        }
        // Refresh on either (a) deadline elapsed or (b) the
        // notify-watcher said the broker DB changed. The watcher
        // shaves the typical refresh latency from ~1s to ~50ms when
        // the platform supports it; on platforms without notify
        // support `take_dirty` always returns false and the
        // deadline path is the only trigger (PR-UI-3 behaviour).
        let dirty = watch.take_dirty();
        if dirty || app.last_refresh.elapsed() >= REFRESH_INTERVAL {
            refresh_with_default_sources(&mut app, &pane_source);
            // Team root drifted (operator launched in a different
            // tree) → swap the watcher to the new state dir.
            let next_dir = app
                .mailbox_db_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            if next_dir != watched_dir {
                watch = Watch::try_new(&next_dir);
                watched_dir = next_dir;
            }
        } else if app.last_pane_refresh.elapsed() >= PANE_REFRESH_INTERVAL {
            // Between full refreshes, keep the detail view live by
            // re-capturing just the focused agent's pane.
            recapture_focused_pane(&mut app, &pane_source);
        }
    }
    Ok(())
}

/// Fast-cadence re-capture of only the focused agent's tmux pane, so
/// the detail view tracks the live session between the heavier 1s full
/// refreshes. A single `capture-pane` subprocess — no team / mailbox /
/// approval / sysinfo work. No-op when no agent is focused.
pub(super) fn recapture_focused_pane<P: PaneSource>(app: &mut App, pane_source: &P) {
    if app.layout == MainLayout::Chat {
        return;
    }
    if let Some(session) = app
        .stream_target_session()
        .filter(|_| app.layout != MainLayout::Chat)
    {
        // #277: skip the 3000-line capture when `detail_buffer` already
        // holds THIS session's content at the pane's current activity ts
        // (this runs up to 10x/s on an idle pane). A focus switch
        // (different session), new output (changed ts), or an unreadable
        // ts (`None`) all fall through to an unconditional capture, so a
        // stale frame from another agent can never linger on screen.
        //
        // #277 follow-up: `#{window_activity}` has whole-SECOND
        // resolution, so while a pane is actively changing — the operator
        // typing >1 key/s in stream-keys, or an agent streaming output —
        // its ts stays pinned to the current second and equals the cached
        // ts. A bare `cached_ts == live_ts` test then skipped every tick
        // within that second, dropping the detail view from ~10Hz to ~1Hz
        // (it looked frozen). So we only trust the cache once the activity
        // is *settled*: `live_ts < now_secs`, i.e. it happened in an
        // earlier whole second. During any sub-second activity
        // `live_ts == now_secs`, so we recapture every tick — and keep
        // doing so until `now_secs` rolls past that second, i.e. for up to
        // ~1s after the pane goes quiet. That tail of idle captures is
        // bounded and cheap; the gate then re-engages and the idle-CPU win
        // returns. (A change landing in the final <100ms of a second
        // followed by immediate idle can be up to ~1s late — bounded by
        // the unconditional 1Hz full refresh, the same staleness ceiling
        // the detail view already relies on.)
        let ts = pane_source.last_activity_secs(&session);
        let now_secs = app.now_secs as u64;
        let already_current = matches!(
            (&app.detail_buffer_activity, ts),
            (Some((cached_session, cached_ts)), Some(live_ts))
                if cached_session == &session
                    && *cached_ts == live_ts
                    && live_ts < now_secs
        );
        if !already_current {
            if let Ok(lines) = pane_source.capture(&session) {
                app.set_detail_buffer(lines);
                app.detail_buffer_activity = ts.map(|t| (session.clone(), t));
            }
        }
    }
    app.last_pane_refresh = Instant::now();
}

fn tick_chat(app: &mut App, worker: &mut Option<(std::path::PathBuf, crate::chat::ChatWorker)>) {
    let active = app.layout == MainLayout::Chat;
    if worker
        .as_ref()
        .is_some_and(|(path, _)| path != &app.mailbox_db_path)
    {
        // Old responses are dropped with their receiver; they cannot cross roots.
        *worker = None;
        app.chat = crate::chat::ChatState::default();
    }
    if worker.is_none() && active {
        *worker = Some((
            app.mailbox_db_path.clone(),
            crate::chat::ChatWorker::new(app.mailbox_db_path.clone()),
        ));
    }
    let Some((_, worker)) = worker else {
        return;
    };
    for _ in 0..crate::chat::QUEUE_CAP {
        let Some(response) = worker.try_receive() else {
            break;
        };
        app.chat.apply(response);
    }
    if active && app.chat.last_refresh.elapsed() >= REFRESH_INTERVAL {
        app.chat.refresh();
    }
    for _ in 0..crate::chat::QUEUE_CAP {
        let Some(request) = app.chat.outbox.pop_front() else {
            break;
        };
        if let Err(request) = worker.try_submit(request) {
            app.chat.outbox.push_front(request);
            break;
        }
    }
}

/// T-199: push the focused agent's inner tmux pane to match the
/// Detail rect teamctl-ui will draw into. No-op when:
///
/// - no agent is focused (nothing to size);
/// - the active main layout isn't Triptych (Wall / MailboxFirst
///   render differently and aren't in scope for this fix; flagged
///   as follow-up surfaces in #199);
/// - the Detail rect is degenerate (zero width or height — the
///   helper returns `None` and we leave the pane alone);
/// - the cached size for this session already matches.
///
/// The cache lives on `App` (`last_synced_pane_sizes`) so the
/// common case (no resize, focused on same agent) is a HashMap
/// lookup, not a subprocess spawn.
pub fn sync_focused_pane_size_to<R: crate::pane_resize::PaneResizer>(
    app: &mut App,
    total_area: ratatui::layout::Rect,
    resizer: &R,
) {
    if !matches!(app.layout, MainLayout::Triptych) {
        return;
    }
    let stream = matches!(app.stage, Stage::StreamKeys);
    let Some(detail) = crate::pane_resize::triptych_detail_area(
        crate::pane_resize::main_areas(total_area)[0],
        app.has_pending_approvals(),
        stream,
    ) else {
        return;
    };
    let Some(session) = app.stream_target_session() else {
        return;
    };
    // `render_detail` wraps the captured pane in a bordered block with a
    // top title (Borders::ALL + `.title(...)`), so it draws into an inner
    // rect 2 cols and 2 rows smaller than `detail`. Size the tmux pane to
    // that INNER area — sizing to the outer rect makes the agent reflow to
    // N rows while we only render N-2, leaving an empty gap at the bottom
    // and nudging the first line down a row.
    let mut inner = detail.inner(ratatui::layout::Margin::new(1, 1));
    if !app.detail_splits.is_empty() {
        let cells = crate::pane_resize::detail_cell_areas(inner, &app.detail_splits);
        let Some(cell) = cells.get(app.selected_split) else {
            return;
        };
        inner = cell.inner(ratatui::layout::Margin::new(1, 1));
    }
    let inner_w = inner.width;
    let inner_h = inner.height;
    if inner_w == 0 || inner_h == 0 {
        return;
    }
    let target = (inner_w, inner_h);
    if !crate::pane_resize::should_sync(&app.last_synced_pane_sizes, &session, target) {
        return;
    }
    if app
        .pane_resize_failures
        .get(&session)
        .is_some_and(|(previous, time)| previous == &target && time.elapsed() < REFRESH_INTERVAL)
    {
        return;
    }
    if resizer.try_resize(&session, target.0, target.1) {
        app.pane_resize_failures.remove(&session);
        app.last_synced_pane_sizes.insert(session, target);
    } else {
        app.pane_resize_failures
            .insert(session, (target, Instant::now()));
    }
}

/// Build the production `BrokerMailboxSource` + `BrokerApprovalSource`
/// from the current team root and run a refresh with all three
/// default sources. Lives here (rather than inline in `run`) so
/// the team-root → DB-path derivation has one home.
fn refresh_with_default_sources<P: PaneSource>(app: &mut App, pane_source: &P) {
    if let Ok(Some(snapshot)) = current_snapshot(app) {
        app.replace_team(snapshot);
    }
    let db_path = app.mailbox_db_path.clone();
    let mailbox_source = BrokerMailboxSource::new(db_path.clone());
    let approval_source = BrokerApprovalSource::new(db_path);
    if let Some(session) = app
        .stream_target_session()
        .filter(|_| app.layout != MainLayout::Chat)
    {
        if let Ok(lines) = pane_source.capture(&session) {
            app.set_detail_buffer(lines);
        }
    } else {
        app.detail_buffer.clear();
    }
    refresh_mailbox(app, &mailbox_source);
    refresh_approvals(app, &approval_source);
    // T-209: refresh the live CPU/RAM numbers on the same 1-second
    // cadence as the rest of App. `refresh_cpu_usage` + `refresh_memory`
    // are the minimal pair — `refresh_all` would also probe disks,
    // networks, processes, and components, none of which the status
    // bar shows. Sub-millisecond cost on modern hardware.
    app.sysinfo.refresh_cpu_usage();
    app.sysinfo.refresh_memory();
    app.last_refresh = Instant::now();
    app.last_pane_refresh = Instant::now();
}

fn current_snapshot(app: &App) -> Result<Option<TeamSnapshot>> {
    if app.team.root.as_os_str().is_empty() {
        TeamSnapshot::discover_and_load()
    } else {
        TeamSnapshot::load(&app.team.root).map(Some)
    }
}
