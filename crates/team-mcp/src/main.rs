//! `team-mcp` — the shared agent mailbox, exposed as an MCP stdio server.
//!
//! Protocol: MCP (JSON-RPC 2.0 over stdio, line-delimited). Protocol version
//! pinned to the workspace constant `team_core::MCP_PROTOCOL_VERSION`.
//!
//! Each agent runs its own `team-mcp` child (spawned by the runtime via
//! `--mcp-config`). All processes point at the same SQLite file. Concurrent
//! writers are handled by SQLite in WAL mode.
//!
//! # Channels delivery
//!
//! When the connected client is Claude Code v2.1.80+ launched with
//! `--channels server:team`, this server pushes every new inbox row as an
//! `notifications/claude/channel` JSON-RPC notification. The runtime injects
//! the payload into the live session as a `<channel source="team">` event,
//! so the agent reacts on arrival without polling. The watcher initialises
//! its high-water mark to the current max inbox id at startup, so unacked
//! pre-existing mail is left for the agent to fetch via `inbox_peek` (the
//! bootstrap prompt directs it to). Other runtimes silently ignore the
//! notification, so emitting unconditionally is safe.
//!
//! Non-claude runtimes (codex, gemini) are strictly request/response over
//! MCP — there is no push mechanism they honour. For those agents the
//! watcher additionally types a short "📬 N new team message(s)" nudge into
//! the agent's tmux pane (the same mechanism `compact_self` uses), batching
//! all rows found in one poll tick into a single nudge. The agent then
//! drains its mailbox via `inbox_peek` / `inbox_read` / `inbox_ack`.

mod rpc;
mod store;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};
use tokio::sync::{Mutex, Notify};

#[derive(Parser)]
#[command(
    name = "team-mcp",
    version,
    about = "MCP server for the teamctl mailbox"
)]
struct Cli {
    /// Fully-qualified agent id as `<project>:<agent>`.
    #[arg(long, env = "TEAMCTL_AGENT_ID")]
    agent_id: String,

    /// Path to the SQLite mailbox database.
    #[arg(long, env = "TEAMCTL_MAILBOX")]
    mailbox: PathBuf,

    /// Tmux session prefix (matches `compose.global.supervisor.tmux_prefix`).
    /// Used by `compact_self` (T-109) to compute the caller's tmux session
    /// name as `<prefix><project>-<role>`. Default matches `team-core`'s
    /// default prefix so a hand-launched MCP server still works on a
    /// stock team.
    #[arg(long, env = "TEAMCTL_TMUX_PREFIX", default_value = "t-")]
    tmux_prefix: String,

    /// T-32b: compose root used to load `attachments:` config and
    /// stage tempfiles under `<root>/state/attachments-staging/`.
    /// Populated by `team-core::render` when an agent's MCP config
    /// is rendered. When unset (hand-launched servers, older
    /// renderer), the `read_attachment` tool returns a "disabled"-
    /// style reject so the agent never silently sees raw bytes.
    #[arg(long, env = "TEAMCTL_COMPOSE_ROOT")]
    compose_root: Option<PathBuf>,
}

/// Poll cadence for the channel watcher. SQLite SELECT against a WAL-mode
/// db with one indexed predicate is sub-millisecond; 500 ms is the
/// "feels instant to a human, costs ~nothing" sweet spot.
const CHANNEL_POLL_MS: u64 = 500;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TEAM_MCP_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let store = store::Store::open(&cli.mailbox)?;
    let mut ctx = tools::Ctx::new(cli.agent_id.clone(), store, cli.tmux_prefix.clone());

    // T-32b: load attachment policy from compose, build the scanner
    // wrapper if configured, sweep stale tempfiles. All best-effort:
    // a missing/unparseable compose drops the team-mcp into the
    // "attachments unavailable" mode rather than failing boot, so
    // the rest of the MCP surface remains live.
    if let Some(compose_root) = cli.compose_root.as_ref() {
        match team_core::compose::Compose::load(compose_root) {
            Ok(compose) => {
                let cfg = compose.global.attachments.clone();
                let scanner = cfg
                    .scanner
                    .as_ref()
                    .map(team_core::attachments::RealScanner::for_spec);
                let dir = team_core::attachments::staging_dir(compose_root);
                let ttl = std::time::Duration::from_secs(cfg.tempfile_ttl_seconds);
                match team_core::attachments::sweep_expired(&dir, ttl) {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(reaped = n, "attachments staging sweep"),
                    Err(e) => tracing::warn!(error = %e, "attachments staging sweep failed"),
                }
                ctx = ctx.with_attachments(tools::AttachmentsCtx {
                    cfg,
                    compose_root: compose_root.clone(),
                    scanner,
                });
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    compose_root = %compose_root.display(),
                    "attachment config: compose load failed; read_attachment will return disabled",
                );
            }
        }
    }

    tracing::info!(
        agent_id = %cli.agent_id,
        mailbox = %cli.mailbox.display(),
        "team-mcp ready",
    );

    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    // Channels notifications must not be sent before the client signals
    // `notifications/initialized` per the MCP lifecycle. The watcher waits
    // on this gate; the request loop trips it.
    let initialized = Arc::new(Notify::new());

    // T-104: lazy inbox delivery is the default. Operators can disable it
    // globally via `TEAM_LAZY_INBOX=0`, which restores the pre-T-104
    // full-body channel notifications. Per-message override stays available
    // through the `/readnow` prefix on bot-routed messages.
    let lazy_inbox = std::env::var("TEAM_LAZY_INBOX")
        .map(|v| !matches!(v.trim(), "0" | "false" | "off" | "no"))
        .unwrap_or(true);

    spawn_channel_watcher(
        ctx.store.clone(),
        ctx.agent_id.clone(),
        ctx.tmux_prefix.clone(),
        stdout.clone(),
        initialized.clone(),
        lazy_inbox,
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<rpc::Request>(trimmed) {
            Ok(req) => {
                if req.method == "notifications/initialized" {
                    // `notify_one()` (not `notify_waiters()`): tokio's
                    // `notify_waiters` only wakes tasks that are *already*
                    // parked on `.notified().await`; if the spawned watcher
                    // task hasn't been polled yet (freshly `tokio::spawn`'d,
                    // tokio scheduling delay), the wake fires into the void
                    // and the watcher blocks forever. macOS runners under
                    // load surfaced this as a flake on
                    // `immediate_message_delivers_full_body_inline` —
                    // 30s budget elapsed with zero notifications because
                    // the watcher never unblocked. `notify_one()` stores a
                    // permit when no waiter is ready, so a late-polled
                    // watcher consumes the buffered permit on its first
                    // `.notified().await` and proceeds.
                    initialized.notify_one();
                }
                if let Some(resp) = rpc::dispatch(&ctx, req).await {
                    let buf = serde_json::to_vec(&resp)?;
                    write_line(&stdout, &buf).await?;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, line = %trimmed, "invalid JSON-RPC");
                let resp = rpc::Response::parse_error(&e.to_string());
                let buf = serde_json::to_vec(&resp)?;
                write_line(&stdout, &buf).await?;
            }
        }
    }
    Ok(())
}

async fn write_line(stdout: &Arc<Mutex<Stdout>>, buf: &[u8]) -> Result<()> {
    let mut out = stdout.lock().await;
    out.write_all(buf).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

fn spawn_channel_watcher(
    store: Arc<store::Store>,
    agent_id: String,
    tmux_prefix: String,
    stdout: Arc<Mutex<Stdout>>,
    initialized: Arc<Notify>,
    lazy_inbox: bool,
) {
    // Session name for the nudge path, derived once — it depends only on
    // the fixed tmux prefix + agent id. `None` when the agent id is
    // malformed (nudges skipped, logged once). The delivery *mode* is NOT
    // latched here: it's re-resolved inside the poll loop, on every tick
    // that found new rows (see there for why).
    let nudge_session = match tools::pane_session(&tmux_prefix, &agent_id) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(error = %e, "channel watcher nudge disabled");
            None
        }
    };
    // High-water mark captured *synchronously* at spawn — before the
    // tokio task is scheduled. Computing it inside the task after
    // `initialized.notified().await` opened a race: any writer that
    // inserted between the test (or operator) sending
    // `notifications/initialized` and the task actually waking would
    // be captured in `last_seen` and never emitted as a channel
    // notification. This was the source of the
    // `immediate_message_delivers_full_body_inline` flake — under
    // load on CI, the 150ms sleep the test used as a barrier could
    // be shorter than the watcher's wake-and-peek latency.
    //
    // Capturing here shrinks the race window to the ~µs between
    // `Store::open` and this peek — small enough that no real writer
    // can plausibly slip in. Any row inserted from this point forward
    // gets a notification, which is the contract callers expect.
    let initial_last_seen: i64 = match store.inbox_peek(&agent_id, 1000) {
        Ok(msgs) => msgs.iter().map(|m| m.id).max().unwrap_or(0),
        Err(e) => {
            tracing::warn!(error = %e, "channel watcher initial peek failed");
            0
        }
    };

    // Test-only barrier (kept out of prod paths because the env var is
    // unset there): widens the "initialized arrives before watcher parks"
    // race window deterministically so a regression test can pin the
    // `notify_one` (buffered-permit) contract on the caller side.
    let pre_notify_sleep_ms: u64 = std::env::var("TEAM_MCP_TEST_WATCHER_PRE_NOTIFY_SLEEP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    tokio::spawn(async move {
        if pre_notify_sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(pre_notify_sleep_ms)).await;
        }
        initialized.notified().await;
        let mut last_seen = initial_last_seen;

        loop {
            tokio::time::sleep(Duration::from_millis(CHANNEL_POLL_MS)).await;
            let msgs = match store.inbox_peek(&agent_id, 100) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "channel watcher peek failed");
                    continue;
                }
            };
            let mut max_id = last_seen;
            let mut fresh = 0usize;
            // First fresh row's (sender, body) feeds the nudge preview so
            // non-claude agents can triage without an inbox_read round-trip.
            let mut first_fresh: Option<(String, String)> = None;
            for m in msgs.iter().filter(|m| m.id > last_seen) {
                fresh += 1;
                if first_fresh.is_none() {
                    first_fresh = Some((m.sender.clone(), m.text.clone()));
                }
                // Emitted for every runtime: claude injects it as a
                // `<channel>` event; non-claude clients ignore unsolicited
                // notifications, so keeping the emit path single-shape is
                // free and leaves the claude behaviour byte-identical.
                let payload = format_channel_event(m, lazy_inbox);
                let buf = match serde_json::to_vec(&payload) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "channel notification serialise failed");
                        continue;
                    }
                };
                if let Err(e) = write_line(&stdout, &buf).await {
                    tracing::warn!(error = %e, "channel notification write failed; aborting watcher");
                    return;
                }
                if m.id > max_id {
                    max_id = m.id;
                }
            }
            // Non-claude delivery: one nudge per poll tick that found new
            // rows (batched, never per-row), typed into the agent's own
            // pane. Fire-and-forget on the blocking pool, log-and-drop on
            // failure — same contract as `compact_self`. A missed nudge is
            // recovered by the next arrival's nudge (the high-water mark
            // advances regardless) and by the agent's own `inbox_peek`
            // habit from the bootstrap prompt.
            //
            // The delivery mode is re-resolved on every tick with rows to
            // deliver — one indexed sqlite read, and only when there's
            // something to announce keeps it near-free. Latching it at
            // spawn was wrong: scoped `teamctl reload <project>` / scoped
            // `up` restart sessions via `render_project_public` without
            // re-running `register_all_public`, so a compose runtime flip
            // CAN outlive this process's spawn-time snapshot. On lookup
            // *error* we fall back to Channel for this tick rather than
            // latching Nudge: for claude that's exactly right (the channel
            // event above already went out), and a codex agent merely
            // misses one nudge — the next tick's nudge or its own
            // `inbox_peek` habit recovers it. A registered-but-unknown
            // runtime string still nudges (see `delivery_mode`).
            if fresh > 0 {
                let mode = match store.runtime_for(&agent_id) {
                    Ok(runtime) => delivery_mode(runtime.as_deref()),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "channel watcher runtime lookup failed — \
                             assuming channel delivery for this tick",
                        );
                        DeliveryMode::Channel
                    }
                };
                if mode == DeliveryMode::Nudge {
                    // `first_fresh` is always Some here (`fresh > 0`), but
                    // matching both keeps the dispatch obviously panic-free.
                    if let (Some(session), Some((sender, body))) =
                        (nudge_session.clone(), first_fresh.take())
                    {
                        tokio::task::spawn_blocking(move || {
                            let line = inbox_nudge_line(&sender, &body, fresh);
                            // Split type-then-submit: codex strands a
                            // same-write `\r` in the composer, so a bare
                            // `send-keys "<line>" Enter` leaves the note
                            // unsent. See tmux_type_and_submit.
                            tools::tmux_type_and_submit(&session, &line, "inbox nudge");
                        });
                    }
                }
            }
            last_seen = max_id;
        }
    });
}

/// How new inbox rows get in front of the served agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryMode {
    /// Claude Code Channels: the stdout notification alone reaches the
    /// session as a `<channel source="team">` event.
    Channel,
    /// Every other runtime treats MCP as strictly request/response and
    /// drops unsolicited notifications, so new mail is announced by
    /// typing a nudge into the agent's tmux pane.
    Nudge,
}

/// Pick the delivery mode for `runtime` (as registered in the mailbox).
/// Only claude-code honours channel pushes; codex, gemini, unknown
/// runtimes, and unregistered agents (`None`) all get the tmux nudge —
/// for a pane that doesn't exist the nudge just fails and is dropped.
/// Lookup *errors* never reach here: the watcher falls back to Channel
/// for that tick at the call site, keeping this pure over the
/// registered value.
fn delivery_mode(runtime: Option<&str>) -> DeliveryMode {
    match runtime {
        Some("claude-code") => DeliveryMode::Channel,
        _ => DeliveryMode::Nudge,
    }
}

/// Argv for the tmux send-keys invocation that announces `new_rows` fresh
/// inbox rows in the agent's pane, carrying the first row's sender and a
/// short body preview so the agent can triage without an `inbox_read`
/// round-trip. Pulled out (like `tools::compact_self_argv`) so unit tests
/// pin the exact arg shape without spinning up tmux. The text lands in
/// the agent's composer as a user message, so it stays short and points
/// at the mailbox tools; the trailing `Enter` keyword makes tmux fire a
/// Return so the runtime actually processes it.
///
/// `sender` and `body` are UNTRUSTED message data typed into a keyboard
/// surface — both go through `pane_safe_line`, and the line always opens
/// with the fixed `📬 ` prefix so no message can put its own first byte
/// on the composer line (a leading `/` would run as a slash command).
fn inbox_nudge_line(sender: &str, body: &str, new_rows: usize) -> String {
    const PREVIEW_CHARS: usize = 80;
    let sender = pane_safe_line(sender);
    let clean = pane_safe_line(body);
    let mut preview: String = clean.chars().take(PREVIEW_CHARS).collect();
    if clean.chars().count() > PREVIEW_CHARS {
        preview.push('…');
    }
    let more = if new_rows > 1 {
        format!(" (+{} more)", new_rows - 1)
    } else {
        String::new()
    };
    let head = if preview.is_empty() {
        // Non-text rows (kind set, empty body) still get a sane line.
        format!("📬 {sender}: new message{more}")
    } else {
        format!("📬 {sender}: \"{preview}\"{more}")
    };
    format!(
        "{head} — call inbox_peek, \
         then inbox_read each meta.id and inbox_ack when handled."
    )
}

/// Sanitize untrusted text for a line *typed* into a tmux pane: collapse
/// every whitespace run (`\n`, `\r`, `\t`, …) to a single space and drop
/// all other control chars — ASCII C0 + DEL (ESC included) and the
/// Unicode C1 range (U+0080–U+009F, the 8-bit escape equivalents).
///
/// Deliberately separate from `notification_stub`'s preview: that one
/// travels as JSON and lands inside a `<channel>` event where newlines
/// and control bytes are inert. The keyboard path is an execution
/// surface — a newline submits the composer, ESC can drive the runtime's
/// TUI, and a line starting with `/` runs as a slash command — so it
/// needs this stricter, pane-safe treatment.
fn pane_safe_line(text: &str) -> String {
    text.chars()
        // Keep whitespace controls (\n, \r, \t) for the collapse below;
        // drop every other control (C0 + DEL + C1) outright.
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the JSON-RPC notification per Claude Code's Channels wire format.
/// `meta` fields surface as XML attributes on the `<channel>` tag the
/// runtime injects into the session.
///
/// Per the Channels reference, `params.meta` is `Record<string, string>`
/// — every value must be a string. Numbers and nulls are silently
/// dropped (or, worse, drop the whole notification), so we serialise
/// `id` / `sent_at` as strings and omit `thread_id` entirely when it is
/// not set.
///
/// T-104: when `lazy_inbox` is true (default) and the row's
/// `delivery_mode` is not `'immediate'`, `params.content` carries a
/// short notification stub instead of the full body. The agent then
/// drills in via `inbox_read(ids)` to fetch + auto-resolve. When
/// `lazy_inbox` is false OR the row was marked immediate (operator used
/// `/readnow `), the full body lands inline as before.
///
/// #254: `kind = 'system'` rows always deliver full-body inline (no
/// lazy stub), regardless of `lazy_inbox` / `delivery_mode` — they are
/// real-time lifecycle signals (drain, startup, rate-limit) the agent
/// must act on without an `inbox_read` round-trip.
fn format_channel_event(m: &store::Message, lazy_inbox: bool) -> Value {
    let mut meta = serde_json::Map::new();
    meta.insert("id".into(), Value::String(m.id.to_string()));
    meta.insert("sender".into(), Value::String(m.sender.clone()));
    meta.insert("recipient".into(), Value::String(m.recipient.clone()));
    meta.insert("sent_at".into(), Value::String(m.sent_at.to_string()));
    if let Some(t) = m.thread_id.as_ref() {
        meta.insert("thread_id".into(), Value::String(t.clone()));
    }
    let immediate = m.delivery_mode.as_deref() == Some("immediate");
    // #254: `system` kind is a lifecycle signal — always full-body
    // inline + real-time, never a lazy stub, regardless of the global
    // lazy-inbox setting or per-row delivery_mode.
    let system = m.kind.as_deref() == Some("system");
    let content = if !lazy_inbox || immediate || system {
        m.text.clone()
    } else {
        meta.insert("lazy".into(), Value::String("1".into()));
        notification_stub(m)
    };
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": {
            "content": content,
            "meta": meta,
        }
    })
}

/// Render the lazy-delivery notification stub for `m`. The agent sees
/// this in its input stream and decides whether to drill in (via
/// `inbox_read`) or ignore.
///
/// Format:
///   DM:      `📬 1 new message from <sender>: "<preview>"`
///   Channel: `📬 1 new message in <channel> from <sender>: "<preview>"`
///
/// Preview is the first 80 characters of `text`; longer text is
/// truncated with a trailing `…`. Non-text rows (`kind` set, empty text)
/// fall back to a kind-aware label so the stub is never an empty quote.
fn notification_stub(m: &store::Message) -> String {
    const PREVIEW_CHARS: usize = 80;
    let trimmed = m.text.trim();
    let kind_label = m.kind.as_deref();
    if trimmed.is_empty() {
        let label = kind_label.unwrap_or("message");
        let location = stub_location(m);
        return format!("📬 1 new {label}{location} from {}", m.sender);
    }
    let mut preview: String = trimmed.chars().take(PREVIEW_CHARS).collect();
    if trimmed.chars().count() > PREVIEW_CHARS {
        preview.push('…');
    }
    let location = stub_location(m);
    format!(
        "📬 1 new message{location} from {sender}: \"{preview}\"",
        sender = m.sender,
    )
}

/// `" in <channel>"` for channel rows, empty string for DMs. Channels
/// land at `recipient = "channel:<project>:<name>"` — we surface the
/// `<name>` segment to the agent (project is implicit from `meta.sender`).
fn stub_location(m: &store::Message) -> String {
    if let Some(rest) = m.recipient.strip_prefix("channel:") {
        let name = rest.split_once(':').map(|(_, n)| n).unwrap_or(rest);
        format!(" in {name}")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_mode_channels_only_for_claude_code() {
        // The channel push is a Claude Code Channels feature; every other
        // runtime is strictly request/response over MCP and needs the
        // tmux nudge. Unknown runtime strings and unregistered agents
        // (`None`) fall to the nudge side too — a nudge into a missing
        // pane is dropped, whereas a channel push to a non-claude runtime
        // is silently ignored and the agent idles forever.
        assert_eq!(delivery_mode(Some("claude-code")), DeliveryMode::Channel);
        assert_eq!(delivery_mode(Some("codex")), DeliveryMode::Nudge);
        assert_eq!(delivery_mode(Some("gemini")), DeliveryMode::Nudge);
        assert_eq!(delivery_mode(Some("some-future-cli")), DeliveryMode::Nudge);
        assert_eq!(delivery_mode(None), DeliveryMode::Nudge);
    }

    #[test]
    fn inbox_nudge_line_batches_count_and_previews() {
        // The first row's sender + preview and the batch remainder are
        // baked into one line. Submission is the split type-then-Enter in
        // tools::tmux_type_and_submit — pinned there, not here.
        assert_eq!(
            inbox_nudge_line("hello:hugo", "standup in 5", 3),
            "📬 hello:hugo: \"standup in 5\" (+2 more) — call inbox_peek, \
             then inbox_read each meta.id and inbox_ack when handled."
        );

        // Exactly one fresh row: no `(+N more)` tail.
        let single = inbox_nudge_line("hello:hugo", "ping", 1);
        assert!(
            !single.contains("more"),
            "single-row nudge must not carry a batch tail: {single}",
        );
    }

    #[test]
    fn inbox_nudge_always_starts_with_the_fixed_mailbox_prefix() {
        // The typed line is an execution surface: a composer line
        // starting with `/` runs as a slash command. The fixed `📬 `
        // prefix guarantees no untrusted sender or body ever supplies
        // the line's first byte.
        let line = inbox_nudge_line("/quit", "/compact", 1);
        assert!(
            line.starts_with("📬 "),
            "nudge must open with the fixed prefix: {line}",
        );
    }

    #[test]
    fn pane_safe_line_collapses_newlines_and_strips_controls() {
        // An embedded `\n/compact\n` must never reach the pane as its
        // own line — newlines submit, and a line starting with `/`
        // executes as a slash command.
        assert_eq!(
            pane_safe_line("please\n/compact\nnow"),
            "please /compact now"
        );
        // ESC (and every other non-whitespace ASCII control, DEL
        // included) is dropped so bodies can't drive the runtime's TUI.
        assert_eq!(pane_safe_line("red\x1b[31malert\x07\x7f"), "red[31malert");
        // Unicode C1 controls (8-bit escape equivalents like CSI U+009B)
        // are dropped too — `is_control` covers them where
        // `is_ascii_control` would let them through.
        assert_eq!(pane_safe_line("a\u{009b}31mb\u{0085}c"), "a31mb c");
        // Tabs, CRs, and runs of mixed whitespace collapse to single spaces.
        assert_eq!(pane_safe_line("a\t\tb\r\n  c"), "a b c");
    }

    #[test]
    fn inbox_nudge_sanitizes_sender_too() {
        // The sender column is data as much as the body is — it must go
        // through the same pane-safe treatment.
        let line = inbox_nudge_line("evil\nname\x1b", "hi", 1);
        assert!(
            line.starts_with("📬 evil name: \"hi\""),
            "sender must come out control-free and single-line: {line}",
        );
    }

    #[test]
    fn inbox_nudge_preview_truncates_multibyte_on_char_boundary() {
        // `chars().take()` truncation must hold for multibyte scalars —
        // no panic, 80 chars plus the ellipsis, never 81.
        let body = "🦀".repeat(200);
        let line = inbox_nudge_line("hello:dev", &body, 1);
        assert!(
            line.contains(&format!("\"{}…\"", "🦀".repeat(80))),
            "preview must cap at 80 chars with an ellipsis: {line}",
        );
        assert!(!line.contains(&"🦀".repeat(81)));
    }

    #[test]
    fn inbox_nudge_empty_body_still_produces_a_sane_line() {
        // Whitespace-only (or kind-only, empty-text) rows must not yield
        // a dangling empty quote.
        let line = inbox_nudge_line("hello:dev", "  \n\t ", 2);
        assert!(
            line.starts_with("📬 hello:dev: new message (+1 more)"),
            "empty body must fall back to a plain announcement: {line}",
        );
        assert!(line.contains("inbox_peek"));
    }
}
