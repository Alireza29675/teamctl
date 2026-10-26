---
title: Getting started
---

Build the workspace source, authenticate one supported runtime CLI, and start the included personal agent:

```sh
git clone git@github.com:Alireza29675/teamctl.git
cd teamctl
cargo build -p teamctl -p team-mcp -p team-bot
export PATH="$PWD/target/debug:$PATH"
codex login
teamctl validate
teamctl up
teamctl serve
```

Choose `claude-code` or `opencode` in `.team/projects/personal.yaml` if you prefer an authenticated runtime you already use. The agent keeps notes and a task list in `workspace/`, resumes its provider session and takes one bounded work turn every fifteen minutes. You can message it immediately in the browser. No Telegram credentials are required.

For a separate project, run `teamctl init my-workspace --yes`, then `cd my-workspace`, `teamctl up` and `teamctl serve`. The generated definition is the same [personal recipe](https://github.com/Alireza29675/teamctl/tree/main/examples/personal-agent).

`teamctl down` stops agents. Stopping the web server or closing its page leaves them running. Chat, assets and runtime state are local under `.team/state/`; personal files stay in `workspace/`.

Continue with the [browser workspace](../guides/web-workspace/), [configuration reference](../reference/team-compose-yaml/) or [native execution and containers](../guides/native-agents/). Published releases may predate this new workspace; use the source build above for `serve`.
