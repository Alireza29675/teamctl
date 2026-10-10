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

The compose root determines the broker database through `broker.path`.
The UI watches that database's directory and polls as a fallback. Message
and approval writes go through the `teamctl` CLI, with subprocess output
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
