//! `teamctl rl-watch <project>:<agent> -- <bin> <args…>`
//!
//! Spawns a runtime binary under a *pseudo-terminal* so it sees a real TTY
//! (interactive Claude Code REPL, Codex, etc. all need this), forwards
//! the wrapper's own stdin into the pty so attached operators can drive
//! the session, copies pty output back to the wrapper's stdout, AND
//! scans each line for the runtime's `rate_limit_patterns`. On a hit:
//!
//! 1. Insert a row into the `rate_limits` table.
//! 2. Run the agent's `on_rate_limit` hook chain (or the global default).
//! 3. Sleep until the captured `resets_at` (with a small jitter) or
//!    `fallback_wait_seconds`.
//! 4. Exit 0 — the surrounding `agent-wrapper.sh` loop respawns the runtime
//!    *after* the limit window has cleared.
//!
//! Without the pty wrap, runtimes detect non-TTY stdio and silently drop
//! into one-shot/print mode, exit immediately, and the wrapper enters a
//! 5-second restart loop -- which was the v0.1 behaviour.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{FixedOffset, Local, NaiveTime, TimeZone, Timelike, Utc};
use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use regex::Regex;
use rusqlite::{params, Connection};
use team_core::compose::{Compose, RateLimitHook};
use team_core::runtimes::RateLimitPattern;

pub fn run(root: &Path, target: &str, runtime_args: &[String]) -> Result<()> {
    let compose = super::load(root)?;
    let Some(handle) = compose.agents().find(|h| h.id() == target) else {
        bail!("no such agent: {target}");
    };
    let runtimes = team_core::runtimes::load_all(&compose.root)?;
    let Some(rt_def) = runtimes.get(&handle.spec.runtime) else {
        bail!(
            "runtime `{}` for agent `{target}` is unknown -- not built in and no `<root>/runtimes/{}.yaml` override found",
            handle.spec.runtime,
            handle.spec.runtime
        );
    };

    if runtime_args.is_empty() {
        bail!("rl-watch needs a runtime command after `--`");
    }
    let bin = &runtime_args[0];
    let bin_args = &runtime_args[1..];

    let patterns = compile_patterns(&rt_def.rate_limit_patterns)?;
    let db_path = compose.root.join(&compose.global.broker.path);

    tracing::info!(
        agent = %target,
        runtime = %handle.spec.runtime,
        "rl-watch starting; {} pattern(s)",
        patterns.len()
    );

    // Open a pty pair sized to our controlling terminal. Inside a tmux
    // pane this is the pane's current size; the SIGWINCH handler below
    // keeps it in sync as the operator attaches/resizes.
    let pty_size = current_winsize();
    let pair = native_pty_system()
        .openpty(pty_size)
        .context("openpty for runtime")?;

    // Build the child command. CommandBuilder doesn't inherit env by default;
    // copy ours through so the runtime sees PATH, HOME, ANTHROPIC_*, etc.
    let mut cmd = CommandBuilder::new(bin);
    for arg in bin_args {
        cmd.arg(arg);
    }
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawn runtime `{bin}` under pty"))?;
    drop(pair.slave); // close our copy of the slave fd

    let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
    let mut writer = pair.master.take_writer().context("take pty writer")?;

    // If our stdin is a TTY, switch it to raw mode so individual keystrokes
    // reach the child immediately (instead of being buffered until newline
    // by line discipline). Restored on drop of the guard.
    let _termios_guard = if std::io::stdin().is_terminal() {
        TermiosGuard::new()
    } else {
        TermiosGuard::noop()
    };

    let child_alive = Arc::new(AtomicBool::new(true));

    // Forward SIGWINCH to the inner pty so the runtime reflows when the
    // tmux pane resizes (operator attach, terminal resize, split, etc.).
    // The slave fd was dropped above, but the master fd is still live
    // here; move it into a dedicated thread and call `resize` whenever
    // the signal handler flags a change. Without this, the TUI inside
    // would stay stuck at whatever size it had at spawn time.
    #[cfg(unix)]
    {
        install_winch_handler();
        let master = pair.master;
        let winch_alive = child_alive.clone();
        thread::spawn(move || {
            // Coalesce the initial state in case the pane size changed
            // between openpty and now (e.g. a client attached during
            // the brief window before we installed the handler).
            let _ = master.resize(current_winsize());
            while winch_alive.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(150));
                if WINCH_PENDING.swap(false, Ordering::SeqCst) {
                    let _ = master.resize(current_winsize());
                }
            }
        });
    }

    // Stdin -> pty writer thread. Exits when stdin hits EOF or child dies.
    let stdin_alive = child_alive.clone();
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        while stdin_alive.load(Ordering::SeqCst) {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if writer.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                Err(_) => break,
            }
        }
    });

    // A bounded reader channel lets us observe a newline-free idle runtime
    // while allowing a short settling window for a split reset capture.
    let (output_tx, output_rx) = std::sync::mpsc::sync_channel(8);
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut scanner = OutputScanner::default();
    let stdout = std::io::stdout();
    let mut hit: Option<(RlEvent, i64, f64)> = None;
    let mut deadline: Option<Instant> = None;

    loop {
        if deadline.is_some_and(|until| Instant::now() >= until) {
            break;
        }
        let output = match deadline {
            Some(until) => output_rx
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .ok(),
            None => output_rx.recv().ok(),
        };
        match output {
            None => break,
            Some(bytes) => {
                {
                    let mut out = stdout.lock();
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
                if let Some(ev) = scanner.feed(&bytes, &patterns) {
                    // Persist the observation while the runtime is still
                    // live. TUIs can idle indefinitely after printing a
                    // limit; waiting for EOF hides the hit forever.
                    let recorded = if let Some((previous, row_id, _)) = hit.as_mut() {
                        if ev.resets_at.is_some() || previous.resets_at.is_none() {
                            update_hit(&db_path, *row_id, &ev).map(|()| {
                                *previous = ev;
                            })
                        } else {
                            Ok(())
                        }
                    } else {
                        record_hit(&db_path, target, &handle.spec.runtime, &ev).map(
                            |(row_id, hit_at)| {
                                hit = Some((ev, row_id, hit_at));
                                deadline = Some(Instant::now() + Duration::from_millis(250));
                            },
                        )
                    };
                    if let Err(error) = recorded {
                        let _ = child.kill();
                        child_alive.store(false, Ordering::SeqCst);
                        let _ = child.wait();
                        return Err(error);
                    }
                }
            }
        }
    }

    // A runtime may exit after a final status line without a newline.
    if hit.is_none() {
        if let Some(ev) = scanner.finish(&patterns) {
            let (row_id, hit_at) = record_hit(&db_path, target, &handle.spec.runtime, &ev)?;
            hit = Some((ev, row_id, hit_at));
        }
    }

    if hit.is_some() {
        let _ = child.kill();
    }
    child_alive.store(false, Ordering::SeqCst);
    let status = child.wait().context("wait runtime")?;

    if let Some((ev, row_id, hit_at)) = hit {
        on_hit(
            &compose,
            &db_path,
            target,
            &handle.spec.runtime,
            &ev,
            row_id,
            hit_at,
        )?;
        return Ok(()); // wrapper re-spawns
    }

    // No rate-limit detected — exit with the runtime's own status code.
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("runtime exited {}", status_str(&status)))
    }
}

mod detector;
mod hooks;
mod terminal;

use detector::{compile_patterns, OutputScanner, RlEvent};
use hooks::{on_hit, record_hit, update_hit};
use terminal::{current_winsize, TermiosGuard};
#[cfg(unix)]
use terminal::{install_winch_handler, WINCH_PENDING};

fn status_str(status: &ExitStatus) -> String {
    if status.success() {
        "0".into()
    } else {
        format!("{}", status.exit_code())
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests;
