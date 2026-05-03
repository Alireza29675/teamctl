---
description: First-run teamctl onboarding — from no-teamctl-installed to a running supervised team in one conversation.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl-init` is the first-run onboarding for teamctl. Seven stages: prerequisites and install (Stage 1), pick a team shape (Stage 2), confirm the proposed org (Stage 3), scaffold `.team/` and reveal the YAML (Stage 4), bring it up (Stage 5), wire Telegram (Stage 6), point at the lifecycle commands (Stage 7).

Read [RULES.md](../RULES.md) before each stage. Voice rails: 1-2 sentences per beat, "experienced reliable coworker", emojis sparingly. Body voice is runtime-neutral. *"Claude Code runtime"* is a fact about the agent and stays; *"Claude reads the file"* is voice drift and goes. Substrate constraints are non-negotiable. The flow is resumable and idempotent — re-running skips anything already done.

## Stage 1 — Detect & install

Probe for prerequisites in this order: `tmux`, `git`, `claude`, `teamctl`. Use `command -v` (or `which`) under `Bash`, one probe per tool. Report inline as a tight bullet:

```
✓ tmux        ✓ git        ✓ claude        ✗ teamctl
```

If all four check out, the prereq line plus one beat moves to Stage 2:

> All four are in place. Ready to set up your team?

No celebration prose, no walls. If `teamctl` is missing, choose an install path by autodetect:

- **macOS with `brew` on PATH**: propose `brew install teamctl`. Confirm before running.
- **Linux, WSL, or macOS without brew**: propose `curl -fsSL https://teamctl.run/install | sh`. Confirm before running.
- **If brew or the curl installer doesn't fit** (sandboxed shell, locked-down corp env, build-from-source preference), use the cargo fallback verbatim:

  > Brew and the curl installer don't fit here. Building from source is the path: `cargo install teamctl team-mcp team-bot` if you've got Rust; otherwise install `rustup` first (https://rustup.rs).

Run the chosen command yourself when the user confirms and the harness allows it; otherwise hand the user the exact line to paste. Either way, verify with `teamctl --version` after install and report the version inline. If the version probe fails, name the error in one line and offer to retry or switch install path — don't restart the stage.

If `tmux`, `git`, or `claude` are the ones missing, name what's missing and the canonical install path for the user's platform (`brew install tmux`, the Claude Code installer, etc.). Don't pretend to install runtimes the plugin can't reasonably manage — surface the gap and pause.

## Stage 2 — What's the team for?

Single beat:

> What kind of work? Pick one or describe yours:

Then the four named defaults, **verbatim**, in this order:

> 1. OSS maintainer — a maintainer + 4 workers (triage, bug-fix, docs, release-manager); pauses for you on releases and merges to main.
> 2. Editorial room — an editor + 3 workers (writer, fact-checker, seo-research); pauses for you before publish.
> 3. Indie studio — a director + 3 workers (designer, writer, playtest-critic); pauses for you before releases or outbound emails.
> 4. Solo triage — a manager + 2 workers (research, inbox); pauses for you before sending anything external.

Then the escape hatch, **verbatim**:

> Or: tell me what your team should look like and I'll scaffold one to fit.

If the user picks 1, 2, 3, or 4, advance to Stage 3 with that named default. The pick is sticky — re-running the command later resumes from this point.

If the user picks the escape hatch and describes a custom team, hold for v1 with this surface:

> v1 ships with the four named defaults; describing your own team in plain English is on the way. Pick one of the four for now and you can edit afterwards with `/teamctl`.

Then reoffer the four picks. One graceful surface, no apology spiral.

## Stage 3 — Propose org

Render the chosen default's org as a **named** ASCII tree. The team is named — never `team-1`, never `default`. Infer the name from the cwd's directory:

- `~/dev/acme-blog` → `Acme blog`
- `~/projects/sidequest-game` → `Sidequest game`

If the cwd name is generic (`workspace`, `project`, `dev`, `code`, `src`, single letter), prompt once with a sensible default for the chosen team type:

> What should I call this team? (`OSS maintainers` / `Editorial desk` / `Studio team` / `Triage desk`)

If the user just hits enter, take the offered default.

The tree maps the chosen default's actual roster from `examples/<name>/.team/projects/<id>.yaml`. Manager on top, "← you talk to this one on Telegram" annotation on the manager box, workers fanning out below.

### OSS maintainer (5 agents)

```
                ┌──────────────┐
                │  maintainer  │ ← you talk to this one on Telegram
                └──────┬───────┘
                       │
       ┌───────────┬───┴────┬─────────────────┐
       │           │        │                 │
  ┌────▼─────┐ ┌───▼────┐ ┌─▼────┐ ┌──────────▼────────┐
  │  triage  │ │bug_fix │ │ docs │ │  release_manager  │
  └──────────┘ └────────┘ └──────┘ └───────────────────┘
```

### Editorial room (4 agents)

```
              ┌──────────────┐
              │ head_editor  │ ← you talk to this one on Telegram
              └──────┬───────┘
                     │
       ┌─────────────┼──────────────────┐
       │             │                  │
  ┌────▼───────┐ ┌───▼──────────┐ ┌─────▼────────┐
  │news_writer │ │ fact_checker │ │ seo_research │
  └────────────┘ └──────────────┘ └──────────────┘
```

### Indie studio (4 agents)

```
              ┌──────────────┐
              │   director   │ ← you talk to this one on Telegram
              └──────┬───────┘
                     │
       ┌─────────────┼──────────────────┐
       │             │                  │
  ┌────▼─────┐ ┌─────▼────┐ ┌───────────▼───────┐
  │ designer │ │  writer  │ │  playtest_critic  │
  └──────────┘ └──────────┘ └───────────────────┘
```

### Solo triage (3 agents)

```
       ┌──────────────┐
       │   manager    │ ← you talk to this one on Telegram
       └──────┬───────┘
              │
       ┌──────┴───────┐
       │              │
  ┌────▼─────┐  ┌─────▼────┐
  │ research │  │  inbox   │
  └──────────┘  └──────────┘
```

Closing line, with the agent count for the chosen default:

```
N Claude Code agents · Opus 4.7 · effort high. ship it?
```

Where N is `5` for OSS maintainer, `4` for editorial room, `4` for indie studio, `3` for solo triage. The runtime descriptor is intentional — the v1 plugin scaffolds Claude Code agents on Opus 4.7 at high effort, per the parent ticket. Sister plugins handle other runtimes.

If the user confirms with "ship it", "yes", "go", or similar, advance to Stage 4. If they push back — wanting to drop a worker, swap a model, route a manager through Slack instead of Telegram — surface this once:

> v1 ships the four named defaults as-is; the `/teamctl` ongoing skill (after init) handles edits. Want to ship as-is and I'll point you at `/teamctl` afterwards?

Take a yes/no. If yes, advance. If no, accept it gracefully and exit; the user can re-run `/teamctl-init` later or hand-author `.team/team-compose.yaml` directly.

## Stage 4 — Init + reveal

This is the moment the plugin commits to disk. Inputs handed off from Stages 2-3: the **chosen default** (one of the four named picks), the **team name** (e.g. "Acme editorial", never `team-1`), and the **cwd** to scaffold into.

The plugin scaffolds `.team/` directly. **Don't shell out to `teamctl init`** — its static `solo` / `blank` templates can't express the four named defaults' richer shape (per-manager Telegram interfaces, scoped channels, full HITL `globally_sensitive_actions`, budget). The four `examples/<folder>/.team/` trees are the canonical golden output; Stage 4 reproduces them byte-for-byte modulo three intentional substitutions (project id, team name, `tmux_prefix`).

### Default-name → example-folder mapping

User-facing skill labels diverge from on-disk folder names for two of the four. Resolve before reading the source tree:

| Skill label (Stage 2) | `examples/<folder>/` | project YAML |
| --- | --- | --- |
| OSS maintainer       | `oss-maintainer`     | `projects/oss.yaml` |
| Editorial room       | `newsletter-office`  | `projects/newsroom.yaml` |
| Indie studio         | `indie-game-studio`  | `projects/studio.yaml` |
| Solo triage          | `solo-triage`        | `projects/triage.yaml` |

The folder-rename tickets parked separately (per parent T-077 clarifications log); the skill maps the label to the folder, no apology surface needed.

**Editorial room asymmetry.** `examples/newsletter-office/.team/team-compose.yaml` lists two projects (`newsroom.yaml` + `blog-site.yaml`) and carries a top-level `interfaces:` email block wired to `newsroom:head_editor`. The "Editorial room" pick maps to the **newsroom project only** (the 4-agent roster the user confirmed in Stage 3). Drop the `blog-site.yaml` entry from the user's `projects:` list. Keep the email-interface block — it's how head_editor is reached in this default and is part of what the user signed up for.

### Derived inputs

- **Project id** — kebab-case slug of the team name. Lowercase, alphanumeric + hyphens, collapse runs of hyphens, trim leading/trailing hyphens. "Acme editorial" → `acme-editorial`. "Side-project triage!" → `side-project-triage`.
- **`tmux_prefix`** — `<project-id>-` (trailing hyphen). Used in the user's `team-compose.yaml`.
- **Project-YAML filename** — `projects/<project-id>.yaml` in the user's tree (the example's filename, e.g. `oss.yaml`, gets renamed to the user's project id).
- **Team display name** — the user's chosen string verbatim, written to the `name:` field in `projects/<project-id>.yaml`.

### Files to write

Read each file from `examples/<folder>/.team/` and write the same content under `<cwd>/.team/`, applying the three substitutions and one filename rename:

```
<cwd>/.team/
├── team-compose.yaml         # copy from example; substitute tmux_prefix + projects: file:
├── projects/<project-id>.yaml # copy from example's projects/<example-id>.yaml; substitute project.id + project.name
├── roles/<role>.md           # one per agent — generated on the fly, see below
├── .env.example              # copy from example verbatim (already canonical — TEAMCTL_TG_<NAME>_* for telegram defaults; NEWSROOM_EMAIL_* for editorial-room)
└── .gitignore                # copy from example verbatim
```

Substitutions are surgical:

- `team-compose.yaml`: change `tmux_prefix:` value (e.g. `oss-` → `acme-editorial-`); change the entry under `projects:` to `- file: projects/<project-id>.yaml` so it points at the user's renamed project YAML.
- `projects/<project-id>.yaml`: change `project.id:` (e.g. `oss` → `acme-editorial`) and `project.name:` (e.g. `OSS Maintainer` → `Acme editorial`). Channels, managers, workers, ACLs, and interfaces stay byte-for-byte.

Everything else — broker block, supervisor type, budget, hitl `globally_sensitive_actions`, channels list, manager/worker definitions, env-var references in `interfaces.telegram.bot_token_env` / `chat_ids_env` — copies verbatim. The example folders already use the canonical `TEAMCTL_TG_<NAME>_TOKEN` / `TEAMCTL_TG_<NAME>_CHATS` shape; no env-var work needed here.

**No plugin-specific markers anywhere.** No `# generated-by:` comments. No skill signatures. No "this file was scaffolded by /teamctl-init" preamble. A user opening `team-compose.yaml` should not be able to tell it came from a plugin (substrate constraint #3).

### Role-prompt generation

For each agent in `projects/<project-id>.yaml` — managers and workers both — generate `roles/<agent-id>.md` on the fly. **Don't copy the example's role prompt verbatim**; the example is inspiration, not a template. Generation runs inside this Claude Code session — read the spine plus the role facts, then write the role prompt directly to disk.

For each agent, supply the model with:

1. **The 8-section spine**, read verbatim from `plugins/claude-code/role-prompt-style.md`. Every generated role prompt has all eight section headers in order: Identity, Mission, Voice, Best practices, Loop, Memory, Boundaries + HITL gates, Hard rules.
2. **Role facts** drawn from the chosen project YAML and the team context:
   - Agent id, agent kind (manager / worker), reports-to relationship, peers in the same project.
   - Channels the agent is on (`can_dm`, `can_broadcast` from the YAML).
   - HITL gates from the team's `globally_sensitive_actions`.
   - Telegram-bound or not (manager only — read `interfaces.telegram` presence).
3. **Substance inspiration** — the corresponding `examples/<folder>/.team/roles/<agent-id>.md`. Read it for *what kind of work this role does*; restate in the user's team's terms (the team name, the chosen default's project framing). The 8-section spine output may diverge in shape from the example's prose; substance should match.
4. **Voice** — default coworker baseline at this stage (slack-style, short, concise, clear, emoji-friendly, "experienced reliable coworker"). Stage 6 regenerates Telegram-bound managers' prompts with custom-voice overrides if the user asks for one; Stage 4 doesn't pre-empt that.

Write the prompt directly to `<cwd>/.team/roles/<agent-id>.md`. No CLAUDE attribution in the file. No "generated by" footer. The prompt should read like a careful human wrote it.

### Validate

Run `teamctl validate` from `<cwd>`. Exit 0 is the gate.

If validate succeeds, advance to the reveal beat.

If validate fails (theoretically shouldn't if the example folders are sound, but defensive):

> Hmm, validate flagged this: `<error verbatim>`. Want me to undo the `.team/` and stop, or leave it for you to inspect?

Surface the error **verbatim** — don't re-format, don't paraphrase, don't massage. The user gets the rollback choice or the inspect choice; honour either. Validation failure here means a plugin bug, and the honest surface is the recovery path.

### Reveal beat

When validate is green, close Stage 4 with the literal text — substrate constraint #2, verbatim required:

> I wrote `.team/team-compose.yaml` for you — open it, everything we just talked about is in there.

Voice rails apply (1-2 sentences, "experienced reliable coworker"). Don't pad with a celebration paragraph; the line stands. Then advance to Stage 5.

## Stage 5 — Run

Single beat:

> Bring it up?

On confirm, run `teamctl up` from `<cwd>`. Parse the output for the agent count and the tmux-prefix-named sessions, then report inline:

```
✓ 5 sessions alive in tmux (oss-maintainer, oss-triage, oss-bug_fix, oss-docs, oss-release_manager)
```

Adapt the count and the names from the chosen default's roster and the project-id's `tmux_prefix`. The bullet is the whole beat — no celebration paragraph after it.

If `teamctl up` fails, surface the error verbatim and offer two paths forward — retry, or look at it together. Don't restart Stages 1-4. Voice rails: 1-2 sentences, "experienced reliable coworker," no apology spiral.

> `teamctl up` errored — here's what came back: `<error>`. Retry, or want to look at it together?

The "look at it together" beat is teammate-flavored on purpose. The user picked a path; if the runtime hiccupped, you're the colleague who debugs it with them, not the wrapper that gives up.

## Stage 6 — Telegram + voice-customize

Single transition beat:

> Reach `<manager>` from anywhere? Let's wire a Telegram bot.

Substitute `<manager>` with the actual user-facing manager's role name from the chosen default (`maintainer` for OSS maintainer, `head_editor` for editorial room, `director` for indie studio, `manager` for solo triage). For teams with multiple user-facing managers — the four named defaults each have one; `/teamctl`-evolved teams may have more — iterate the stage per manager.

For each user-facing manager:

### Pre-existing-interface pre-check

Before invoking `teamctl bot setup`, check the manager's `interfaces:` block in the project YAML. If the manager already has a non-Telegram interface wired (editorial room's `head_editor` ships with `email` pre-wired per the newsroom asymmetry; future templates may carry others), surface a polite beat first:

> `<manager>` reaches you via `<existing-interface>` already. Want to add Telegram on top of that, or skip Telegram for this one?

If the user picks "add Telegram", proceed with bot wiring — the manager will be reachable via both interfaces. If the user picks "skip Telegram", mark this manager as Stage-6-skipped-for-Telegram and continue straight to the voice-customize sub-beat (which still fires; voice customization is interface-independent). Don't silently override and don't silently skip — both paths are user-driven.

### Bot wiring (wraps `teamctl bot setup`)

The wrap routes through `team_core::yaml_edit` so the YAML edits to `interfaces.telegram` preserve comments and blank-line clusters elsewhere in the file. Steps:

1. **Name the bot in BotFather.** Walk the user through DMing `@BotFather` on Telegram, picking a name (e.g. `Acme editorial bot`), and getting a token back. Hand them the exact command/text to send.
2. **Capture the token.** When the user pastes it, write it to `<cwd>/.team/.env` under `TEAMCTL_TG_<NAME>_TOKEN` (canonical pattern; matches the `bot_token_env` reference already in the manager's `interfaces.telegram` block from Stage 4). `.env` is gitignored — the token stays local.
3. **Capture the chat id.** Tell the user: *"Send `/start` to your new bot from Telegram, then I'll capture your chat id."* Read the chat id off the bot's update stream and write it to `TEAMCTL_TG_<NAME>_CHATS` in the same `.env`.
4. **Verify.** Confirm the manager can be reached — one quick test message round-trip is enough. Voice: *"Bot's live — try DMing `<manager>` and you should see the response come back."*

If `teamctl bot setup` fails for a specific manager (BotFather rate limit, network blip, mis-pasted token), surface the error verbatim and offer two paths — retry that manager, or skip Telegram on that manager and wire it later via `/teamctl`. Don't restart the stage; don't skip silently.

> `teamctl bot setup` for `<manager>` didn't take — here's the error: `<error>`. Retry that one, or skip Telegram on `<manager>` and wire it later via `/teamctl`?

The "wire it later via `/teamctl`" path is the substrate constraint #4 receipt at a failure moment — the user isn't trapped, every action is reversible by hand or by the ongoing skill.

### Voice-customize sub-beat

After bot wiring (or after the user picked "skip Telegram" in the pre-check), ask once per manager getting Telegram:

> Want to customize `<manager>`'s voice, or use the default?

**Default voice** (no regen): slack-style, short, concise, clear, emoji-friendly, proactive in sharing and checking with stakeholders, "experienced reliable coworker." Stage 4 already generated `roles/<manager>.md` with this voice; if the user picks default, you're done with voice for this manager.

**Custom voice** (triggers regen): ask the user to describe what they want, anchored on dimensions, not examples:

> Describe the voice you want — a sentence or two is plenty. Tone, formality, emoji use — whatever you want different.

Capture the user's overrides. Re-run T-077-C's role-prompt-gen mechanism for THIS manager only, with the custom-voice override merged into section 3 (Voice) of the 8-section spine. Sections 1, 2, 4-8 stay as Stage 4 generated them. Overwrite `<cwd>/.team/roles/<manager>.md` with the regenerated prompt — same path, same 8-section structure, custom Voice section.

If the user has multiple managers getting Telegram, the sub-beat fires per manager. After the first ask, drop the long default-voice description on subsequent prompts — *"Want to customize `editor`'s voice too, or use the default?"* is enough; the user already knows what default means.

Per project-owner directive: the voice-customize sub-beat only fires for managers getting Telegram. Workers stay on the Stage-4 default voice. Managers that the user opted to skip Telegram on still get the sub-beat — voice customization is interface-independent.

## Stage 7 — UI + lifecycle

Three lines, in order, each on its own beat:

> Watch them work: `teamctl ui`

> Reload after edits: `teamctl reload`

> Full restart (state preserved): `teamctl down && teamctl up`

Then the closing beat — verbatim, no paraphrase:

> You're done. The team is yours.

Don't pad. The closing line is the load-bearing voice surface of the whole onboarding; it's the screenshot. Hand the keys back and stop.

## Substrate constraints recap

In case any stage tempts a shortcut:

1. The plugin name on the marketplace card is **`teamctl`** — internal command names stay descriptive (`/teamctl-init`, `/teamctl`).
2. The reveal beat ("I wrote `.team/team-compose.yaml` for you…") fires at the end of Stage 4 — verbatim. Don't pre-empt it earlier; don't restyle it later.
3. The `.team/` output Stage 4 produces is byte-for-byte identical to a hand-authored team — no plugin-only state, no generated-by markers.
4. Every action this command takes is reproducible by hand-editing YAML afterwards.

> See `.team/tasks/2026-05-03-teamctl-cc-plugin/SLICING.md` for the full slice plan.
