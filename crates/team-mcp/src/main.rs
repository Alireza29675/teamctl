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
    stdout: Arc<Mutex<Stdout>>,
    initialized: Arc<Notify>,
    lazy_inbox: bool,
) {
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
            for m in msgs.iter().filter(|m| m.id > last_seen) {
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
            last_seen = max_id;
        }
    });
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
    let content = if !lazy_inbox || immediate {
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
