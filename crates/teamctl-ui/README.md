# teamctl-ui

Interactive terminal UI for `teamctl`.

```bash
cargo install teamctl-ui     # or: cargo build -p teamctl-ui
teamctl-ui                   # from inside any directory under a `.team/` tree
```

`Tab` cycles pane focus, `q` opens quit confirmation, `?` opens help, and
`t` opens the tutorial. `@` composes a direct message and `!` selects a
channel for a broadcast. In the detail pane, `Ctrl+E` toggles keyboard
streaming to the selected tmux session; `Esc` is forwarded to the agent.

Press `c` from the dashboard for Chat: durable conversations, a wrapped
transcript, and a multiline composer. `Tab` cycles conversations, history,
and composer; `Shift+Tab` reverses. Conversation rows resume existing
threads; `+ local` rows create or resume a chat with another agent. Return
to the previous dashboard with `Ctrl+D`, or quit with `Ctrl+Q`.

In the composer, `q`, `c`, `@`, and `!` are text. `Enter` adds a newline;
`Esc` then `Enter`, `Alt+Enter`, or `Ctrl+Enter` sends. Two Esc presses
focus the conversation list. `PageUp`/`PageDown` and the mouse wheel
browse the transcript; `End` follows new replies. Historical views stay
anchored when replies arrive and show a new-message count. Drafts and
scroll positions stay with their conversation while switching threads.
Failed sends retain the draft and retry with the same request identity;
editing a failed draft creates a new submission. A successful send never
clears text edited while that send was pending.

CLI and UI share the `local` / `operator` conversation for each agent.
Remote adapter threads are readable, with their adapter and peer shown,
and send through their own messenger. Removed agents retain readable
history while their composer is disabled. Transcript history survives
agent inbox cleanup and client restart; unsent drafts are session-local.
The UI loads SQL pages of 100 messages and holds at most 1,000 messages
or 2 MiB of text per visited thread. Chat database operations run on a
bounded background queue, outside the terminal event loop.

The compose root determines the broker database through `broker.path`.
The UI watches that database's directory and polls as a fallback. Message
and dashboard approval/message writes go through the `teamctl` CLI, with subprocess output
captured so it cannot overwrite the terminal display.

`src/app.rs` preserves the public application API. Its internal modules
own state construction, navigation, compose/approval actions, event
dispatch, runtime refresh, and rendering. The interactive terminal and
snapshot tests share one renderer. Detail-cell geometry is shared with
tmux resize synchronization in `pane_resize.rs`.

From the repository root, run `cargo test -p teamctl-ui` for unit,
interaction, CLI, and snapshot coverage. The real tmux resize check is
opt-in: `cargo test -p teamctl-ui resize_window_actually -- --ignored`.
It creates a temporary detached session and removes it afterward.

The crate is excluded from the workspace's `default-members`: plain
`cargo build` does not pull the ratatui dep tree. Build it with
`-p teamctl-ui` or `--workspace`.
