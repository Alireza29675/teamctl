# TeamCTL terminal UI

Inspect your TeamCTL agents, conversations and approval requests in the terminal. TeamCTL is an open-source platform for defining and running autonomous teams of agents; `teamctl-ui` is its dashboard.

```bash
cargo install teamctl-ui     # or: cargo build -p teamctl-ui
teamctl-ui                   # from inside any directory under a `.team/` tree
```

You can also open the dashboard with `teamctl ui`. Closing it leaves the team's background sessions running while the host remains awake.

The crate is excluded from the workspace's `default-members`: plain
`cargo build` does not pull the ratatui dep tree. Build it with
`-p teamctl-ui` or `--workspace`.

[Documentation](https://teamctl.run/getting-started/) · [CLI reference](https://teamctl.run/reference/teamctl/) · [Source](https://github.com/Alireza29675/teamctl)
