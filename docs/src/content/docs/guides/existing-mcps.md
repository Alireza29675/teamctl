---
title: Existing MCP servers
description: Community and Anthropic MCP servers that work with teamctl today, with paste-ready mcps blocks.
---

teamctl gives any agent its own MCP servers through a per-agent `mcps:` block, alongside the built-in `team` mailbox bus. This page curates servers that work with teamctl **today**, with paste-ready snippets, so you don't have to rediscover which packages still exist or how they launch.

The list moves fast. Treat every entry as a snapshot (dated below), and confirm the upstream link before you lean on it.

## What "works with teamctl" means

The contract is narrow and worth understanding before you paste anything:

- **stdio only.** A server fits teamctl if it launches as a local process (a `command` plus `args`) and speaks MCP over stdio. teamctl renders each entry into the runtime's MCP config next to the `team` server. Remote, SSE, or streamable-HTTP-only servers do **not** fit yet (the `url` transport is deferred, see [What doesn't fit yet](#what-doesnt-fit-yet)).
- **Three fields, passed through verbatim.** An `mcps:` entry is `command`, `args`, and `env`, nothing else. teamctl does no `${VAR}` expansion of its own.
- **Secrets come from `.team/.env`.** Reference a variable as `"${TOKEN}"` in `env:` and put the real value in `.team/.env` (gitignored). See [Secrets](#secrets) for how the value actually reaches the server. Never inline a literal token.
- **Paths are cwd-relative.** A filesystem-style server's directory args resolve against the agent's project `cwd`, not the team root.
- **`team` is reserved** for the mailbox bus teamctl injects for you. You can't name a server `team`.

**How each entry is verified.** Claude Code is the reference runtime. Every server below carries a badge: *validated* means the `mcps:` block passes `teamctl validate` and matches a launch shape teamctl already ships; *desk-checked* means the launch shape is confirmed against the upstream project but wasn't run here. Neither badge proves the upstream server itself still works end to end, so confirm the upstream link and run one `teamctl up` before you rely on a server. teamctl hands the same config to Codex and Gemini, but third-party-server acceptance there isn't verified, so treat non-Claude runtimes as desk-checked.

## The 2024 server list is out of date

If you came here looking for the classic "filesystem / GitHub / Slack / Postgres / Puppeteer" reference servers: that set is a 2024 mental model. In early 2025 Anthropic split the [reference-servers repo](https://github.com/modelcontextprotocol/servers) down to a small active core, the official **GitHub** npm package was **deprecated (April 2025)** in favor of a container, and the Postgres and Puppeteer reference servers were **archived**. The snippets below are the current shapes.

## Where `mcps:` goes

A server is declared **per agent**, inside a project file (`projects/<id>.yaml`), which uses the integer schema `version: 2`, not the `"2.0.0"` string the top-level `team-compose.yaml` uses. Pasting a block under the wrong version line is an easy mistake to make.

```yaml
# projects/research.yaml
version: 2

workers:
  researcher:
    runtime: claude-code
    mcps:
      filesystem:
        command: npx
        args: ["-y", "@modelcontextprotocol/server-filesystem", "."]
```

After editing, run `teamctl reload` (or `up`) to hand the block to the agent on its next spawn. Run `teamctl validate` first if you want a dry-run check.

## Curated servers

Every entry is a snapshot as of **2026-06-12**. Confirm the upstream link before relying on it, and run one `teamctl up` to confirm end to end.

### Filesystem: read/write files in a sandboxed dir

Anthropic reference server, actively maintained. _Validated._

```yaml
mcps:
  filesystem:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "."]
```

The trailing arg is the allowed directory, resolved against the agent's `cwd`; pass more to widen the sandbox. Upstream: [`@modelcontextprotocol/server-filesystem`](https://www.npmjs.com/package/@modelcontextprotocol/server-filesystem).

### Git: inspect and commit a local repo

Anthropic reference server (Python, via `uvx`). _Validated._

```yaml
mcps:
  git:
    command: uvx
    args: ["mcp-server-git", "--repository", "."]
```

Upstream: [`mcp-server-git`](https://github.com/modelcontextprotocol/servers/tree/main/src/git).

### Fetch / Memory / Time / Sequential-thinking: the small active core

The remaining actively-maintained reference servers, all stdio. _Desk-checked._

```yaml
mcps:
  fetch:
    command: uvx
    args: ["mcp-server-fetch"]
  memory:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-memory"]
  time:
    command: uvx
    args: ["mcp-server-time"]
  sequential-thinking:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-sequential-thinking"]
```

`fetch` retrieves and converts web pages to markdown, `memory` is a knowledge-graph scratchpad, `time` does timezone conversions, and `sequential-thinking` is a structured-reasoning scratchpad. Upstream: [modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers).

### GitHub: issues, PRs, code search

The npm package is dead; the current path is GitHub's official **container**, run over stdio. _Validated_ (this is the shape the `oss-maintainer` example ships).

```yaml
mcps:
  github:
    command: docker
    args: ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_PERSONAL_ACCESS_TOKEN}"
```

Needs Docker on the host and `GITHUB_PERSONAL_ACCESS_TOKEN` in `.team/.env`. The `-e NAME` in `args` tells docker to forward the env var into the container; `env:` is where teamctl supplies it. The hosted remote-HTTP GitHub server does **not** fit (see below). Upstream: [github/github-mcp-server](https://github.com/github/github-mcp-server).

### Browser automation: Playwright

Microsoft's actively-shipped Playwright MCP. Prefer it over the archived Puppeteer reference server. _Desk-checked._

```yaml
mcps:
  playwright:
    command: npx
    args: ["-y", "@playwright/mcp@latest"]
```

Upstream: [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp).

### Postgres: query a database (community)

The Anthropic Postgres reference server is archived; the maintained community replacement is `crystaldba/postgres-mcp`. _Desk-checked, community-maintained; verify before production use._

```yaml
mcps:
  postgres:
    command: uvx
    args: ["postgres-mcp", "--access-mode=restricted"]
    env:
      DATABASE_URI: "${DATABASE_URI}"
```

Upstream: [crystaldba/postgres-mcp](https://github.com/crystaldba/postgres-mcp).

### Slack: read and post (community)

The Anthropic Slack reference server is archived; `slack-mcp-server` is a maintained community option with a stdio transport. _Desk-checked, community-maintained._

```yaml
mcps:
  slack:
    command: npx
    args: ["-y", "slack-mcp-server@latest", "--transport", "stdio"]
    env:
      SLACK_MCP_XOXP_TOKEN: "${SLACK_MCP_XOXP_TOKEN}"
```

Upstream: [korotovsky/slack-mcp-server](https://github.com/korotovsky/slack-mcp-server).

## Secrets

Every server that needs a credential follows the same pattern. Reference, don't inline:

```yaml
# projects/research.yaml
mcps:
  github:
    command: docker
    args: ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
    env:
      GITHUB_PERSONAL_ACCESS_TOKEN: "${GITHUB_PERSONAL_ACCESS_TOKEN}"
```

```bash
# .team/.env  (gitignored, never commit this)
GITHUB_PERSONAL_ACCESS_TOKEN=ghp_your_real_token
```

`teamctl up` loads `.team/.env` into its own environment, and each agent's session inherits it, so the real value is present in the environment when the server launches. The `"${VAR}"` you write in `env:` is a reference to that variable; for container servers, the `-e NAME` arg is what forwards it into the container.

## What doesn't fit yet

- **Remote / hosted servers.** Anything that only speaks SSE or streamable-HTTP (including GitHub's hosted remote server and most SaaS-hosted MCPs) has no home in `mcps:` today. The `url` / `headers` transport is deferred until a concrete need lands. Use the stdio variant where one exists (as with GitHub's container above).
- **Non-Claude runtimes, unverified.** teamctl hands the same config to Codex and Gemini, but whether each third-party server is accepted there hasn't been verified for this page. Assume Claude Code unless you've tested otherwise.

## Keeping this current

MCP servers churn: packages get renamed, deprecated, or archived. If a snippet here stops working, check the upstream link first (the launch shape usually moved), and please open an issue so this page can be corrected. Every entry is dated; treat anything older than a few months as needing a re-check.
