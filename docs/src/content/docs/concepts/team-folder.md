---
title: The `.team/` folder
---

A repo with a teamctl team carries a `.team/` directory at its root, the
same way a git repo carries `.git/`. teamctl walks up from the current
working directory looking for the nearest `.team/team-compose.yaml`, so
every subcommand "just works" wherever you are inside the tree.

## Layout

```
my-repo/
├── .team/
│   ├── team-compose.yaml      # global: broker, supervisor, hitl, …
│   ├── projects/
│   │   └── <id>.yaml          # one or more
│   ├── roles/
│   │   └── <agent>.md         # role prompts referenced by agents
│   ├── runtimes/              # optional overrides for shipped runtimes
│   ├── .env                   # gitignored — secrets and chat-ids
│   ├── .env.example           # checked-in template
│   ├── .gitignore             # auto-generated
│   └── state/                 # gitignored — mailbox + rendered artifacts
└── … rest of the repo
```

## Discovery

```
$ pwd
/Users/me/work/my-repo/src/deep/nested
$ teamctl ps
…shows agents, even though we're four levels deep
```

Resolution order:

1. `--root <path>` flag.
2. `TEAMCTL_ROOT` env var.
3. Active context (see `teamctl context`).
4. Walk up from CWD looking for `.team/team-compose.yaml`.
5. Walk up looking for a flat `team-compose.yaml` (legacy fallback;
   prints a deprecation warning).

## Multiple teams on one machine

Use `teamctl context`:

```
$ teamctl context ls
*  newsroom            /Users/me/work/news/.team
   startup             /Users/me/work/startup/.team

$ teamctl context use startup
$ teamctl ps                  # now shows the startup team
```

`teamctl up` auto-registers the current root as a context the first
time it runs.

## Bootstrapping a new team

```
teamctl init                   # interactive
teamctl init --template solo --yes
teamctl init --template blank --yes --project my-thing
```

Templates are baked into the binary — `init` works offline.

## Env / secrets

`.team/.env` is the only place secrets live. `.env.example` lists every
variable the compose tree references with placeholder values; commit it.
`.env` itself is gitignored.

```
$ teamctl env
VAR                              STATE     REFERENCED FROM
TEAMCTL_TELEGRAM_TOKEN           set       interfaces[tg-main].config.bot_token_env
TEAMCTL_TELEGRAM_CHATS           UNSET     interfaces[tg-main].config.authorized_chat_ids_env

$ teamctl env --doctor
1 required env var(s) unset
```

`teamctl up` invokes `--doctor` implicitly and refuses to start when
anything required is missing.

## Inspection

```
teamctl ps                     # table: agent, runtime, state, inbox, last activity
teamctl mail <agent>           # this agent's inbox
teamctl mail --all             # every agent's inbox depth + sample
teamctl tail <agent> -f        # live message stream
teamctl inspect <agent>        # full snapshot of one agent
teamctl logs <agent>           # tmux pane scrollback
teamctl attach <agent>         # tmux attach (read-only)
teamctl attach <agent> --rw    # writable, asks you to retype the agent name
teamctl exec <agent> -- ls     # run a command in the agent's CWD with its env
teamctl shell <agent>          # interactive shell with the agent's env loaded
teamctl approvals              # pending HITL requests
teamctl bridge ls              # cross-project bridges
```

The old `status`, `pending`, `bridge list`, etc. commands still work —
they're aliases.

## Migrating from a flat layout

```
mkdir .team
git mv team-compose.yaml projects roles runtimes .team/   # whatever exists
```

Anything that used to live alongside `team-compose.yaml` moves into
`.team/`. State directories (`state/`) and `.env` files should already
be gitignored at the repo root — recreate the gitignore inside
`.team/` (`teamctl init` provides one).
