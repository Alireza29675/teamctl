# teamctl-ui

Interactive terminal UI for `teamctl`.

```bash
cargo install teamctl-ui     # or: cargo build -p teamctl-ui
teamctl-ui                   # from inside any directory under a `.team/` tree
```

**Keys (PR-UI-1 scaffold):** `Tab` to cycle focus across the three panes, `q`
to quit (with confirm). `?` and `t` are reserved for the help overlay and
onboarding tutorial that land in later stacked-PRs (PR-UI-7).

The crate is excluded from the workspace's `default-members`: plain
`cargo build` does not pull the ratatui dep tree. Build it with
`-p teamctl-ui` or `--workspace`.
