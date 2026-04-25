# ADR 0004 — `.team/` folder, first-run UX, and the management surface

- Status: **accepted** (implemented in v0.2.0)
- Date: 2026-04-25
- Author: Alireza
- Reviewers:

## Context

Today, running teamctl in a real project means scaffolding a directory by
hand: `team-compose.yaml`, a `projects/` tree, `roles/*.md`, an `.env`,
optional `runtimes/` overrides, and a shadow `state/` that the tool
generates. Examples in `examples/` show the shape, but a first-time user
must read the docs, copy an example, and stitch it into their codebase.

Three friction points:

1. **No convention for "this repo has a teamctl team"**. A reader looking
   at a repo can't tell at a glance whether it has a team, where its
   files live, or what's gitignored.
2. **First-time setup is a manual copy job**. The `init` story does not
   exist yet; the docs say "copy `examples/hello-team`".
3. **The management surface is thin**. `teamctl status` is a 5-column
   table. Inspecting an agent's mailbox, attaching to its tmux pane,
   tailing a single thread, switching between multiple teams on one host
   — none of these have first-class commands.

The goal of this ADR is to close those gaps with the same taste Docker
and kubectl have: small, sharp, composable verbs that reward muscle
memory and never require a wizard once you've done the thing once.

## Decision

### 1. The `.team/` convention

Every repository that wants a teamctl team carries a top-level `.team/`
directory, exactly the way every git repo carries a `.git/`. The layout:

```
my-repo/
├── .team/
│   ├── team-compose.yaml      # global (broker, supervisor, hitl, ...)
│   ├── projects/
│   │   └── <id>.yaml
│   ├── roles/
│   │   └── <agent>.md
│   ├── runtimes/              # optional overrides for the shipped runtimes
│   │   └── codex.yaml
│   ├── hooks/                 # optional scripts referenced by rate-limit /
│   │   └── on-publish.sh      #          HITL run-actions
│   ├── .env                   # gitignored — secrets and chat-ids
│   ├── .env.example           # checked-in template
│   ├── .gitignore             # auto-generated; covers state/, .env
│   └── state/                 # gitignored — mailbox + rendered artifacts
│       ├── mailbox.db
│       ├── envs/
│       ├── mcp/
│       └── applied.json
└── … rest of the repo
```

#### Discovery

`teamctl` walks up from the current working directory looking for the
nearest `.team/team-compose.yaml`. The first match wins. Identical to
`git`'s `.git/` discovery; identical to Docker Compose's behaviour with
modern compose files. Override with `-C <path>` or `TEAMCTL_ROOT`.

#### Why a dot-folder

- **It signals "infrastructure"** the same way `.git/` does. A reader
  immediately understands this is operational state, not source.
- **One folder is easier to keep right than five**. Today a teamctl
  repo can scatter `runtimes/`, `roles/`, `projects/`, `team-compose.yaml`
  across the root. Consolidating under `.team/` makes ownership visible.
- **Tooling can ignore it** by default. A docs-build or test-runner
  glob doesn't accidentally pick up role prompts.

#### Backward compatibility

`teamctl validate` continues to accept a flat layout (today's `examples/`
shape) so existing configurations don't break. Discovery prefers `.team/`
when both are present; a `WARN` tells the user the flat layout is
deprecated and prints the exact `git mv` to migrate.

### 2. First-run UX

```
$ cd my-repo
$ teamctl init
```

`init` is interactive but every prompt has a sane default and a
non-interactive equivalent. Flow:

1. **Pick a template**:
   - `solo`         — one manager, one dev, Claude Code only.
   - `multi-runtime`— one manager + Codex worker + Gemini researcher.
   - `newsroom`     — head editor + writers + fact-checker (mirrors
     `examples/newsletter-office/newsroom/`).
   - `startup`      — founder + PM + eng-lead + IC + researcher.
   - `market-desk`  — chief + macro + equities + crypto + quant-risk.
   - `blank`        — empty `team-compose.yaml` + an empty
     `projects/main.yaml` you'll fill in.
2. **Project id and human name** — defaults derived from the repo name.
3. **Pick interfaces** — CLI only (default), Telegram, both. Picking
   Telegram triggers the pairing flow in §4.
4. **Confirm preview** — render the proposed `.team/` tree, show diff,
   confirm.
5. **Write files** + run `teamctl validate` + (optionally) `teamctl up`.

Non-interactive equivalent for scripts:

```
teamctl init --template solo --interface cli --yes
teamctl init --template newsroom --interface telegram --yes \
  --telegram-token "$TG_TOKEN" --telegram-chat-id 12345678
```

Either way, the result is a `.team/` tree with a `.env.example`, a
`.gitignore`, and a one-line `README.md` inside `.team/` pointing at the
docs.

### 3. Env vars and secrets

#### Single source of truth

`.team/.env` is the only place secrets live. It is gitignored. Auto-sourced
by `teamctl up` and every subcommand that needs it. A `.env.example`
checked in alongside lists every variable the compose tree references,
with placeholder values and one-line descriptions.

#### YAML never holds a secret

Every place in compose that "wants" a secret takes a `*_env:` field with
the env-var name, not the value:

```yaml
interfaces:
  - type: telegram
    name: tg-main
    config:
      bot_token_env: TEAMCTL_TELEGRAM_TOKEN
      authorized_chat_ids_env: TEAMCTL_TELEGRAM_CHATS
```

The validator rejects raw token-shaped strings (`AAH...`, `sk-...`,
`xoxb-...`, etc.) in YAML with an error pointing at the line and
suggesting the `*_env` form.

#### `teamctl env`

```
$ teamctl env
TEAMCTL_TELEGRAM_TOKEN  set      (****fk2A)
TEAMCTL_TELEGRAM_CHATS  set      (12345678)
PAGER_WEBHOOK           UNSET    used by rate_limits.hooks[pager]
NEWSROOM_EMAIL_USER     set      (newsroom@example.com)
NEWSROOM_EMAIL_PASS     UNSET    used by interfaces[head-editor-mail]
```

Doctor mode:

```
$ teamctl env --doctor
✗ TEAMCTL_TELEGRAM_CHATS is set to "0" — not a valid chat id
✗ PAGER_WEBHOOK is unset and rate_limits.hooks[pager] is in default_on_hit
2 issues
```

`up` invokes `--doctor` and refuses to start if anything fails.

### 4. Telegram bot pairing — safe by default

The riskiest setup step. We make it boring.

```
$ teamctl bot pair tg-main
1. Open Telegram. Search for the bot you created via @BotFather.
   Paste its token here:
   token > 123456789:AAH-…

2. Now message the bot. Anything. e.g. "ping".
   Waiting … (60s timeout)
   ✓ Got message from chat 75473051 ("Alireza S.").

3. Confirm: bind tg-main to chat 75473051? [Y/n] y

✓ Wrote TEAMCTL_TELEGRAM_TOKEN and TEAMCTL_TELEGRAM_CHATS to .team/.env
✓ Sent "teamctl bot paired" reply — confirm you received it. [Y/n] y
✓ Done. Restart: teamctl restart bot
```

Properties:

- **Token never typed in shell history**. `bot pair` reads token via
  `read -s` style (terminal raw mode, no echo).
- **Chat id is auto-discovered** from the user's first message. No
  manual lookup of `@userinfobot`.
- **One-chat default**. The pairing UI binds exactly one chat id. To
  add more, edit `.team/.env`. The bot adapter still rejects everyone
  else.
- **Verifies bidirectional**. The bot must successfully reply to the
  user before pairing finalizes. If the chat is restricted, this fails
  loudly instead of silently.
- **No group chats by default**. The bot adapter rejects updates from
  groups unless explicitly allowed via `allow_groups: true`.

### 5. Inspection — Docker/kubectl-shaped verbs

| Command | Purpose |
|---|---|
| `teamctl ps` | Wide table: project, agent, runtime, state, inbox depth, last-active, today USD. Replaces today's `status`. |
| `teamctl ps -A` | Across every `.team/` registered as a context. |
| `teamctl ps <agent>` | Single-row detail. |
| `teamctl top` | Live-refreshing TUI of `ps` (think `htop` for agents). |
| `teamctl inspect <agent>` | Full snapshot: rendered env, MCP config, role prompt path, last 20 messages, last 5 cost rows, today's rate-limit hits. JSON via `-o json`. |
| `teamctl logs <agent>` | Pane scrollback (last ~3000 lines). |
| `teamctl tail <agent> [-f]` | Stream new messages addressed to / from this agent. `-f` follows. |
| `teamctl mail <agent>` | Inbox table (id, sender, summary, age). |
| `teamctl mail <agent> --thread <id>` | Thread view. |
| `teamctl mail --all` | Every agent's unread inbox depth + sample. |
| `teamctl history <agent>` | Chronological message log; supports `--since`. |
| `teamctl approvals` | Pending HITL requests (replaces `teamctl pending`). |
| `teamctl rate-limits` | Recent hits, who, when, when-they-clear. |
| `teamctl bridge ls / log / open / close` | (today's; rename `list` → `ls` for consistency). |
| `teamctl budget` | (today's). |

#### Output: every list command supports `-o json | yaml | wide`

Same idiom as `kubectl get -o`. Defaults to a human-pretty table.

#### Filters

`-l label=value` for project filtering once we have a `labels:` map on
agents (out of scope for this ADR; flagged as a follow-up).

### 6. Attach and exec

```
teamctl attach <agent>           # tmux attach (read-only by default)
teamctl attach <agent> --rw      # allow keyboard input — dangerous, off by default
teamctl exec <agent> -- ls -la   # run a command in the agent's CWD with its env
teamctl shell <agent>            # interactive shell in the agent's CWD with its env
```

`exec` and `shell` are the difference between debugging and guessing.
A worker fails to find a file → `teamctl shell dev1` and `ls` from
exactly the same place the agent is looking. Same env, same CWD.

`attach` defaults to read-only because muscle-memory typing in a tmux
pane is how 4 a.m. mistakes happen. `--rw` opt-in feels right.

### 7. Multiple teams on one machine — `teamctl context`

Borrowed from `docker context` and `kubectl config`. A "context" is a
named pointer to a `.team/` root.

```
teamctl context ls                     # lists registered contexts
teamctl context use newsroom           # default context for subsequent commands
teamctl context add newsroom ~/work/newsroom
teamctl context rm newsroom
teamctl context current
```

`teamctl up` from inside a `.team/`-bearing repo registers that path as a
context automatically (named after the repo's basename, deduped). The
CLI's resolution order:

1. `-C <path>` flag.
2. `TEAMCTL_ROOT` env.
3. `TEAMCTL_CONTEXT` env (looks up the named context).
4. The current context as set by `teamctl context use`.
5. Discovery from CWD.

### 8. Status badges in the prompt (optional, future)

Like `kube-ps1` — `teamctl status --short` outputs `(team:newsroom 4↑0!)` so
shells can show "current context, agents up, pending approvals". Not in
this ADR, but the design assumes someone will write it.

### 9. Errors that help

Every error tells you the next move.

```
✗ TEAMCTL_TELEGRAM_TOKEN is unset.
  Referenced from: .team/team-compose.yaml:42 (interfaces[tg-main])
  Fix:   teamctl bot pair tg-main
  Or:    add TEAMCTL_TELEGRAM_TOKEN=… to .team/.env

✗ tmux session a-newsroom-head_editor not found.
  Fix:   teamctl up
  Or, if up is failing: teamctl logs newsroom:head_editor (looks at last run)
```

### 10. Naming — alignment pass

The current CLI grew organically. We rationalize on this rollout:

| Today | New | Reason |
|---|---|---|
| `teamctl status` | `teamctl ps` | Docker familiarity. Keep `status` as alias. |
| `teamctl pending` | `teamctl approvals` | Plural noun, consistent with `bridges`, `rate-limits`. Keep `pending` as alias. |
| `teamctl bridge list` | `teamctl bridge ls` | `ls` reads faster. Keep `list`. |
| `teamctl send` | `teamctl mail send` | Group send + read under `mail`. Keep `send`. |

**No flags removed. No commands deleted. Aliases for the old shapes.** A
user with the old habit doesn't have to retype anything. New users learn
the new vocabulary.

## Consequences

### Wins

- A teamctl repo is *visibly* a teamctl repo, the way a git repo is
  visibly a git repo.
- First-time setup is one command (`teamctl init`) plus optional bot
  pairing. No copy-paste of an example tree.
- Inspection is verb-noun and predictable. New users can guess
  commands; experienced users can pipe everything through `-o json`.
- Secrets have one home and one shape. Token-in-yaml is rejected at
  validate time, not in production.

### Costs

- Migration noise. Every existing example needs `mv * .team/` — easy,
  but it's a churn commit. Mitigation: support both layouts during
  the deprecation window.
- `teamctl init` is a real piece of work — interactive prompting,
  template registry, jinja-ish substitution for project ids and chat
  names. Probably a quarter of the codebase by line count.
- The `bot pair` flow needs Telegram polling for ~60 s in `teamctl`
  itself, which means linking in `teloxide` (currently only `team-bot`
  has it). Acceptable.
- `attach --rw` is a foot-gun even with the warning. We mitigate with
  a "type the agent name to confirm" prompt.

### Anti-goals

- **No daemon**. No `teamctld`. teamctl remains a stateless CLI over a
  SQLite mailbox + tmux. The moment we add a daemon, ops complexity
  doubles and we lose the "it's just files" property.
- **No web UI in this ADR**. A read-only TUI (`teamctl top`) covers 80%
  of the dashboard need without a long-running process or auth surface.
- **No multi-tenant/RBAC**. teamctl is for one human + their team(s).
  Multi-user is a different product.

## Open questions

1. **`.team/` vs `.teamctl/`**? `.team` is shorter and more humane;
   `.teamctl` is unambiguous. Lean `.team`. Override-able via a global
   config knob `TEAMCTL_DIRNAME`?
2. **`bot pair` as a teamctl subcommand or a `team-bot` subcommand?**
   Pairing fits with the bot, but users don't think about which crate
   they're calling. Lean: `teamctl bot pair`, with `teamctl` shelling
   out to `team-bot --pair` under the hood.
3. **Init template registry — local-only or remote?** Locally-shipped
   templates are simple and reliable. A remote registry (`teamctl init
   --from github:Alireza29675/team-templates/newsroom`) is much more
   powerful. Lean: ship a small set locally; document the remote path
   for v0.3.
4. **Should `teamctl init` add a hook in `package.json` / `Makefile` /
   the repo's README** to tell future readers how to start the team?
   Lean: yes, but only with `--integrate` opt-in.
5. **Per-agent `labels:` for filtering** — kubectl-style. Out of scope
   here; flagged as a follow-up.
6. **`teamctl logs --since 2h`** — consistent with `kubectl logs`?
   Probably yes. Easy to add once the message store has a time filter.
7. **State directory**: keep `.team/state/` repo-local, or move to
   `~/.local/share/teamctl/<context>/`? Repo-local makes everything
   self-contained and trivially backup-able. Cross-context shells share
   nothing accidentally. Lean: repo-local stays.

## Out of scope (intentionally)

- Multi-host orchestration.
- Web dashboard.
- Per-user RBAC, audit ACLs beyond what bridges + HITL already log.
- A formal plugin system. Hooks (`hooks/*.sh`) cover the customization
  points we've actually needed so far.

## Migration plan (when this is approved)

1. Implement `.team/` discovery alongside the flat layout. Both work;
   `.team/` preferred.
2. Build `teamctl init` with the template set above.
3. Move every example under `examples/*` to a `.team/`-shaped layout.
   Update READMEs.
4. Implement `ps`, `mail`, `tail`, `inspect`, `attach`, `exec`, `shell`,
   `context`, `env [--doctor]`. Preserve every existing command as an
   alias.
5. Implement `teamctl bot pair` with the polling flow.
6. Document the new shape in `docs/concepts/team-folder.md` and a
   "v0.2 migration guide".
7. Cut v0.2.0. The flat-layout deprecation warning starts here.
8. Remove flat-layout support in v1.0.

Estimated cost: ~7 days of focused work for a single implementer.

---

Review notes (please leave inline):

- [ ]
- [ ]
- [ ]
