//! Local chat clients share the durable conversation repository with adapters.

mod interactive;
mod transcript;

#[cfg(test)]
mod tests;

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use team_core::chat::{self, ChatMessage, Conversation, Repository};
use team_core::compose::Compose;

#[derive(clap::Args)]
pub struct Args {
    /// Agent ID or conversation ID for interactive chat.
    pub target: Option<String>,
    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(Subcommand)]
pub enum Action {
    /// Open a local conversation and enqueue a message.
    Send {
        target: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
        /// Reuse the same ID when retrying an uncertain submission.
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read a bounded transcript page, independent of agent inbox cleanup.
    History {
        target: String,
        #[arg(long, conflicts_with = "after")]
        before: Option<i64>,
        #[arg(long)]
        after: Option<i64>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Stream additions and delivery-status revisions in the latest 200 messages.
    Follow {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// List local and external conversations.
    List {
        #[arg(long)]
        json: bool,
    },
}

pub fn run(root: &Path, args: Args) -> Result<()> {
    if args.target.is_some() && args.action.is_some() {
        bail!("choose interactive chat or a chat subcommand");
    }
    let compose = Compose::load_read_only(root)?;
    let db = compose.root.join(&compose.global.broker.path);
    if !db.exists() {
        bail!("chat broker is not initialized; run `teamctl up` first");
    }
    let conn = chat::open(db)?;
    let repo = Repository::new(&conn);
    match args.action {
        None => {
            let target = args
                .target
                .context("usage: teamctl chat <agent>, or chat send/history/list")?;
            let conversation = resolve(&repo, &compose, &target)?;
            interactive::run(&conn, &conversation)
        }
        Some(Action::Send {
            target,
            text,
            request_id: supplied,
            json,
        }) => {
            let conversation = resolve(&repo, &compose, &target)?;
            if conversation.adapter_id != "local" || conversation.peer_id != "operator" {
                bail!("external conversations are read-only here; send from their messenger");
            }
            let id = supplied.unwrap_or_else(request_id);
            let message = repo.send_local(&conversation.id, &text, &id)?;
            if json {
                println!("{}", serde_json::to_string(&message)?);
            } else {
                println!(
                    "queued · {} · message {} · conversation {}",
                    conversation.agent_id, message.id, conversation.id
                );
            }
            Ok(())
        }
        Some(Action::History {
            target,
            before,
            after,
            limit,
            json,
        }) => {
            let conversation = resolve(&repo, &compose, &target)?;
            let rows = repo.history(&conversation.id, before, after, limit)?;
            if json {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                for row in rows {
                    print_message(&row, &conversation);
                }
            }
            Ok(())
        }
        Some(Action::List { json }) => {
            let conversations = repo.list_conversations(200)?;
            if json {
                println!("{}", serde_json::to_string(&conversations)?);
            } else {
                for c in conversations {
                    println!(
                        "{}  {}  {}  {}",
                        c.id,
                        c.agent_id,
                        c.adapter_id,
                        safe_text(&c.title)
                    );
                }
            }
            Ok(())
        }
        Some(Action::Follow { target, json }) => {
            let conversation = resolve(&repo, &compose, &target)?;
            follow(&repo, &conversation, json)
        }
    }
}

fn resolve(repo: &Repository<'_>, compose: &Compose, target: &str) -> Result<Conversation> {
    match repo.get_conversation(target) {
        Ok(conversation) => return Ok(conversation),
        Err(chat::Error::NotFound(_)) => {}
        Err(error) => return Err(error.into()),
    }
    let candidates: Vec<_> = compose
        .agents()
        .filter(|agent| agent.id() == target || (!target.contains(':') && agent.agent == target))
        .collect();
    match candidates.as_slice() {
        [agent] => Ok(repo.open_local(&agent.id())?),
        [] => bail!("unknown conversation or agent: {target}"),
        _ => bail!("ambiguous agent: use a canonical <project>:<agent> ID"),
    }
}

pub(super) fn request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "cli-{}-{nanos}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn safe_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

pub(super) fn print_message(row: &ChatMessage, conversation: &Conversation) {
    println!("{}", message_text(row, conversation));
}

pub(super) fn message_text(row: &ChatMessage, conversation: &Conversation) -> String {
    let author = match row.direction {
        chat::Direction::Human
            if conversation.adapter_id == "local" && conversation.peer_id == "operator" =>
        {
            "you"
        }
        chat::Direction::Human => "human",
        chat::Direction::Agent => "agent",
    };
    let status = match row.status {
        chat::DeliveryStatus::Queued => "queued",
        chat::DeliveryStatus::Sent => "sent",
        chat::DeliveryStatus::Failed => "failed",
        chat::DeliveryStatus::Received => "received",
    };
    let mut text = format!(
        "[{author} · {} · {status}] {}",
        row.id,
        safe_text(&row.text)
    );
    if let Some(error) = &row.error {
        text.push_str(&format!("\n  delivery: {}", safe_text(error)));
    }
    text
}

fn follow(repo: &Repository<'_>, conversation: &Conversation, json: bool) -> Result<()> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    let result = (|| {
        let mut transcript = transcript::Transcript::new(200, 200);
        let mut stdout = io::stdout().lock();
        while !stop.load(Ordering::Relaxed) {
            for row in transcript.poll(repo, &conversation.id)? {
                if json {
                    writeln!(stdout, "{}", serde_json::to_string(&row)?)?;
                } else {
                    writeln!(stdout, "{}", message_text(&row, conversation))?;
                }
            }
            stdout.flush()?;
            std::thread::sleep(Duration::from_millis(300));
        }
        Ok(())
    })();
    signal_hook::low_level::unregister(signal);
    result
}
