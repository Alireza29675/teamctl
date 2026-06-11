//! `teamctl budget-record <project>:<agent>`
//!
//! Records one budget ledger row from outside the runtime, driven by the
//! Claude Code `Stop` hook (rendered by `team-core::render`). The `budget`
//! table has a reader (`teamctl budget`) and a schema, but nothing was ever
//! INSERTing cost rows — so `USD-24H` always read `$0.00`. This is the writer.
//!
//! The `Stop` hook delivers `{session_id, cwd, transcript_path, ...}` on
//! stdin — token usage is NOT in that payload. We read the session transcript
//! at `transcript_path` (a JSONL file), sum the token usage of the just-finished
//! turn (the `assistant` lines after the last `user` line), price the tokens
//! ourselves (the transcript carries tokens, never usd), and INSERT one row.
//!
//! Fire-and-forget: any missing input degrades to a silent `Ok(())`. The hook
//! command also carries a trailing `|| true`, so this must never error a stop.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection};

/// Runtime label stored on every budget row this writer emits. The `Stop`
/// hook is claude-code only (`render_claude_settings` returns None otherwise),
/// so a constant is exact rather than a guess.
const RUNTIME: &str = "claude-code";

/// Bounded retry for the transcript read. The `Stop` hook can fire before
/// Claude Code has flushed the turn's last JSONL line, so a first read may see
/// an empty file or a truncated trailing line. Retry a few times, briefly, then
/// give up — best-effort, never blocking the stop for long.
const READ_ATTEMPTS: usize = 5;
const READ_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Summed token usage for one user-turn, plus the model that produced it.
/// Defaults to all-zero / empty model so a malformed transcript prices to 0.
#[derive(Debug, Default, PartialEq)]
struct TurnUsage {
    input_tok: i64,
    output_tok: i64,
    cache_creation_tok: i64,
    cache_read_tok: i64,
    usd: f64,
}

pub fn run(root: &Path, agent: &str) -> Result<()> {
    // Read the Stop hook payload from stdin. Empty stdin (e.g. invoked by hand)
    // is not an error: there's simply nothing to record.
    let stdin = std::io::read_to_string(std::io::stdin()).unwrap_or_default();
    if stdin.trim().is_empty() {
        tracing::debug!(agent, "budget-record: empty stdin, nothing to record");
        return Ok(());
    }

    let transcript_path = match serde_json::from_str::<serde_json::Value>(&stdin)
        .ok()
        .and_then(|v| {
            v.get("transcript_path")
                .and_then(|p| p.as_str())
                .map(str::to_string)
        }) {
        Some(p) => p,
        None => {
            tracing::debug!(agent, "budget-record: no transcript_path in hook payload");
            return Ok(());
        }
    };

    let jsonl = match read_transcript(Path::new(&transcript_path)) {
        Some(j) => j,
        None => {
            tracing::warn!(
                agent,
                transcript_path,
                "budget-record: transcript unreadable, skipping"
            );
            return Ok(());
        }
    };

    let usage = sum_last_turn_usage(&jsonl);

    let compose = super::load(root)?;
    let db_path = compose.root.join(&compose.global.broker.path);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&db_path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    team_core::mailbox::ensure(&conn)?;

    // `project_id` is the part before the `:`; `agent_id` keeps the full
    // `<project>:<agent>` id, matching the budget reader's `project_id` filter.
    let project_id = agent.split(':').next().unwrap_or(agent);
    record_cost(&conn, project_id, agent, &usage, now())?;

    // Quiet by default: this runs on every clean stop, and a hook's stdout can
    // be surfaced into the agent's context. The recorded row is the durable
    // signal; a success line would be per-stop noise.
    tracing::debug!(agent, usd = usage.usd, "budget-record: recorded cost");
    Ok(())
}

/// Read the transcript, retrying a bounded number of times if it looks
/// unflushed (empty, or a trailing line that doesn't parse as JSON yet).
/// Returns `None` only if the file never became readable.
fn read_transcript(path: &Path) -> Option<String> {
    for attempt in 0..READ_ATTEMPTS {
        if let Ok(content) = std::fs::read_to_string(path) {
            if transcript_is_complete(&content) {
                return Some(content);
            }
        }
        if attempt + 1 < READ_ATTEMPTS {
            std::thread::sleep(READ_RETRY_DELAY);
        }
    }
    // Last-ditch: return whatever is there now (possibly truncated). The
    // per-line parse in `sum_last_turn_usage` tolerates a bad trailing line,
    // so a partial transcript still yields a best-effort sum rather than nothing.
    std::fs::read_to_string(path).ok()
}

/// A transcript is "complete enough" when it has at least one non-blank line
/// and its last non-blank line parses as JSON — the signal that Claude Code
/// finished flushing the turn's final entry.
fn transcript_is_complete(content: &str) -> bool {
    match content.lines().rfind(|l| !l.trim().is_empty()) {
        Some(last) => serde_json::from_str::<serde_json::Value>(last).is_ok(),
        None => false,
    }
}

/// Sum the token usage of the current turn from a transcript's JSONL.
///
/// A `Stop` fires once per user-turn, but a turn has several `assistant`
/// messages (the tool-use back-and-forth). The current turn is every
/// `assistant` line AFTER the last `user`-type line; lines from prior turns are
/// ignored. Each assistant message is priced by its OWN `message.model`, so a
/// turn that mixes models (rare, but possible) sums correctly. Missing fields
/// default to 0 / unknown, which prices to 0.
fn sum_last_turn_usage(jsonl: &str) -> TurnUsage {
    // Collect parsed lines once; we need the index of the last `user` entry
    // before summing the `assistant` entries that follow it.
    let entries: Vec<serde_json::Value> = jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect();

    let entry_type = |v: &serde_json::Value| {
        v.get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string()
    };

    // Start of the current turn: just after the last GENUINE user prompt. Claude
    // Code stores tool RESULTS as `type:"user"` entries too (their content is a
    // `tool_result` block), and in a tool-heavy turn those vastly outnumber real
    // prompts. Keying the boundary on any `type:"user"` line would land on the
    // last tool-result mid-turn and sum almost nothing (often zero) — the exact
    // $0.00 bug this writer exists to fix. So the boundary is the last user entry
    // whose content is NOT a tool_result. If there's none, the whole transcript
    // is the turn (start at 0).
    let is_real_prompt = |v: &serde_json::Value| {
        if entry_type(v) != "user" {
            return false;
        }
        match v.get("message").and_then(|m| m.get("content")) {
            // A content array with any `tool_result` block is a tool output, not
            // a prompt. String content (or any other shape) is a real prompt.
            Some(serde_json::Value::Array(blocks)) => !blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")),
            _ => true,
        }
    };
    let turn_start = entries
        .iter()
        .rposition(is_real_prompt)
        .map_or(0, |i| i + 1);

    let mut usage = TurnUsage::default();
    for entry in &entries[turn_start..] {
        if entry_type(entry) != "assistant" {
            continue;
        }
        let message = match entry.get("message") {
            Some(m) => m,
            None => continue,
        };
        let model = message
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        let u = message.get("usage");
        let tok = |key: &str| -> i64 {
            u.and_then(|u| u.get(key))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        let input = tok("input_tokens");
        let output = tok("output_tokens");
        let cache_creation = tok("cache_creation_input_tokens");
        let cache_read = tok("cache_read_input_tokens");

        usage.input_tok += input;
        usage.output_tok += output;
        usage.cache_creation_tok += cache_creation;
        usage.cache_read_tok += cache_read;
        usage.usd += price_usd(model, input, output, cache_creation, cache_read);
    }
    usage
}

/// Price one assistant message's token usage in USD.
///
/// Published Anthropic list prices as of 2026-06; update when Anthropic
/// reprices. Rates are USD per 1,000,000 tokens, matched by substring so id
/// variations ("claude-opus-4-8", "claude-3-5-sonnet-…") still resolve.
/// Cache tokens are billed off the input rate: cache reads at 0.1x, cache
/// creation (writes) at 1.25x. Unknown models price to 0.0 so the budget
/// column reads 0 rather than a confidently-wrong number.
fn price_usd(
    model: &str,
    input_tok: i64,
    output_tok: i64,
    cache_creation_tok: i64,
    cache_read_tok: i64,
) -> f64 {
    // (input, output) USD per 1M tokens.
    let (input_rate, output_rate) = if model.contains("opus") {
        (15.0, 75.0)
    } else if model.contains("sonnet") {
        (3.0, 15.0)
    } else if model.contains("haiku") {
        (1.0, 5.0)
    } else {
        (0.0, 0.0)
    };

    let per_million = |tokens: i64, rate: f64| (tokens as f64) * rate / 1_000_000.0;

    per_million(input_tok, input_rate)
        + per_million(output_tok, output_rate)
        + per_million(cache_read_tok, input_rate * 0.1)
        + per_million(cache_creation_tok, input_rate * 1.25)
}

/// Insert one budget ledger row. `input_tok` carries the prompt tokens plus
/// both cache buckets (the schema has no cache columns, and they're priced
/// into `usd` already), so the stored counts reconcile with the billed cost.
fn record_cost(
    conn: &Connection,
    project_id: &str,
    agent_id: &str,
    usage: &TurnUsage,
    observed_at: f64,
) -> Result<()> {
    let input_tok = usage.input_tok + usage.cache_creation_tok + usage.cache_read_tok;
    conn.execute(
        "INSERT INTO budget (project_id, agent_id, runtime, usd, input_tok, output_tok, observed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            project_id,
            agent_id,
            RUNTIME,
            usage.usd,
            input_tok,
            usage.output_tok,
            observed_at
        ],
    )?;
    Ok(())
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a schema-bootstrapped in-memory db so the test exercises the real
    /// `budget` table shape, not a hand-rolled subset.
    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        team_core::mailbox::ensure(&conn).expect("bootstrap schema");
        conn
    }

    #[test]
    fn sum_last_turn_usage_sums_only_the_current_turn() {
        // A prior turn (user + assistant), then the current turn: a genuine
        // prompt, an assistant message, a TOOL-RESULT (also stored as
        // `type:"user"` by Claude Code), then another assistant message. The
        // current turn is everything after the genuine prompt: BOTH assistant
        // messages, with the mid-turn tool-result NOT resetting the boundary
        // (the real-transcript shape — tool-results vastly outnumber prompts).
        // The earlier turn's assistant line is ignored.
        let jsonl = r#"
{"type":"user","message":{"role":"user","content":"old"}}
{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":999,"output_tokens":999}}}
{"type":"user","message":{"role":"user","content":"current"}}
{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":200}}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}
{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":40,"output_tokens":20}}}
"#;
        let usage = sum_last_turn_usage(jsonl);
        assert_eq!(
            usage.input_tok, 140,
            "100 + 40: both current-turn assistant msgs, tool-result is not a boundary"
        );
        assert_eq!(usage.output_tok, 70, "50 + 20 from the current turn only");
        assert_eq!(usage.cache_creation_tok, 10);
        assert_eq!(usage.cache_read_tok, 200);
        // Priced as sonnet: input 3/M, output 15/M, cache_read 0.3/M, cache_creation 3.75/M.
        let expected = price_usd("claude-sonnet-4-6", 100, 50, 10, 200)
            + price_usd("claude-sonnet-4-6", 40, 20, 0, 0);
        assert!(
            (usage.usd - expected).abs() < 1e-9,
            "usd should sum each assistant message priced by its own model: {} vs {expected}",
            usage.usd
        );
    }

    #[test]
    fn sum_last_turn_usage_tolerates_missing_fields_and_bad_lines() {
        // No usage block, a partial usage block, and a non-JSON trailing line
        // (the unflushed-tail case). All default to 0 rather than panicking.
        let jsonl = r#"
{"type":"user","message":{"role":"user"}}
{"type":"assistant","message":{"model":"claude-opus-4-8"}}
{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":10}}}
{not valid json
"#;
        let usage = sum_last_turn_usage(jsonl);
        assert_eq!(usage.input_tok, 10);
        assert_eq!(usage.output_tok, 0);
        assert_eq!(usage.cache_creation_tok, 0);
        assert_eq!(usage.cache_read_tok, 0);
    }

    #[test]
    fn price_usd_matches_each_tier_by_substring() {
        // 1M input + 1M output, priced per tier. Substring match resolves the
        // model regardless of id shape.
        assert!((price_usd("claude-opus-4-8", 1_000_000, 1_000_000, 0, 0) - 90.0).abs() < 1e-9);
        assert!((price_usd("claude-sonnet-4-6", 1_000_000, 1_000_000, 0, 0) - 18.0).abs() < 1e-9);
        assert!((price_usd("claude-3-5-haiku", 1_000_000, 1_000_000, 0, 0) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn price_usd_unknown_model_is_zero() {
        // An unrecognized model prices to 0.0 — the column reads 0 rather than
        // a confidently-wrong number.
        assert_eq!(
            price_usd("gpt-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000),
            0.0
        );
    }

    #[test]
    fn price_usd_prices_cache_tokens_off_the_input_rate() {
        // Opus input rate is 15/M: cache reads bill at 0.1x (1.5/M), cache
        // creation at 1.25x (18.75/M). 1M of each => 1.5 + 18.75 = 20.25.
        let usd = price_usd("claude-opus-4-8", 0, 0, 1_000_000, 1_000_000);
        assert!(
            (usd - 20.25).abs() < 1e-9,
            "cache pricing: 1M creation (18.75) + 1M read (1.5): {usd}"
        );
    }

    #[test]
    fn record_cost_inserts_one_row_with_summed_tokens() {
        let conn = db();
        let usage = TurnUsage {
            input_tok: 100,
            output_tok: 50,
            cache_creation_tok: 10,
            cache_read_tok: 200,
            usd: 1.23,
        };
        record_cost(&conn, "alpha", "alpha:dev", &usage, 456.0).expect("record cost");

        let (project_id, agent_id, runtime, usd, input_tok, output_tok, observed_at): (
            String,
            String,
            String,
            f64,
            i64,
            i64,
            f64,
        ) = conn
            .query_row(
                "SELECT project_id, agent_id, runtime, usd, input_tok, output_tok, observed_at \
                 FROM budget",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .expect("query the inserted row");

        assert_eq!(project_id, "alpha");
        assert_eq!(agent_id, "alpha:dev");
        assert_eq!(runtime, "claude-code");
        assert!((usd - 1.23).abs() < 1e-9);
        // input_tok rolls in both cache buckets so the stored count reconciles
        // with the billed cost: 100 + 10 + 200 = 310.
        assert_eq!(input_tok, 310);
        assert_eq!(output_tok, 50);
        assert_eq!(observed_at, 456.0);
    }
}
