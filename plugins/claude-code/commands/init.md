---
description: First-run teamctl onboarding — from no-teamctl-installed to a running supervised team in one conversation.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl:init` is the first-run onboarding for teamctl. Seven stages: prerequisites and install (Stage 1), a discovery conversation that surfaces the user's domains (Stage 2), confirm the proposed org (Stage 3), scaffold `.team/` and reveal the YAML (Stage 4), bring it up (Stage 5), wire Telegram (Stage 6), point at the lifecycle commands (Stage 7).

Read [RULES.md](../RULES.md) before each stage. Voice rails: 1-2 sentences per beat, "experienced reliable coworker", emojis sparingly. Body voice is runtime-neutral. *"Claude Code runtime"* is a fact about the agent and stays; *"Claude reads the file"* is voice drift and goes. Substrate constraints are non-negotiable. The flow is resumable and idempotent — re-running skips anything already done.

> **The shape of Stage 2 matters.** This skill does not hand users a template menu. It walks them through a discovery conversation that surfaces the *domains* in their work — the things with their own state, history, and decisions that compound over time. The cut is by domain ownership, not by job function. Read [`docs/src/content/docs/concepts/teams.md`](../../../docs/src/content/docs/concepts/teams.md) before tuning Stage 2 prose; the methodology there is canonical.

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

  > Brew and the curl installer don't fit here. Building from source is the path: `cargo install teamctl teamctl-ui team-mcp team-bot` if you've got Rust; otherwise install `rustup` first (https://rustup.rs).

Run the chosen command yourself when the user confirms and the harness allows it; otherwise hand the user the exact line to paste. Either way, verify with `teamctl --version` after install and report the version inline. If the version probe fails, name the error in one line and offer to retry or switch install path — don't restart the stage.

If `tmux`, `git`, or `claude` are the ones missing, name what's missing and the canonical install path for the user's platform (`brew install tmux`, the Claude Code installer, etc.). Don't pretend to install runtimes the plugin can't reasonably manage — surface the gap and pause.

## Stage 2 — Discover the domains

This stage is a conversation, not a quiz. The user names *things* in their work; you sharpen and stress-test each one; together you arrive at the set of domains that earn a persistent agent. No template menu. No multiple-choice surface. Read on for the verbatim openers, the sharpening passes, the stress tests, the two-gate validation, and the two fallback paths.

### 2a. Open the conversation

Single beat, verbatim:

> What things in your work have their own state and history — things that change while you're not looking?

Wait for the user to answer in their own words. Don't list examples. Don't offer multiple-choice. The question is the only thing on screen.

If the user names one or more candidates, advance to **2b. Sharpen**.

If the user struggles, says "I don't know" or "I'm not sure what you mean," reach for the second primary question, verbatim:

> Different angle — what do you keep re-explaining to yourself every time you context-switch back to it?

Wait again. If the user still can't surface anything, advance to the **can't-name-anything fallback** in section 2f.

### 2b. Sharpen each candidate

For each candidate the user names, run one sharpening pass. Pick the pass that fits the candidate's shape — don't run them all on the same candidate; that's a quiz.

- **If the candidate sounds substrate-shaped** (a database, a file system, an API endpoint, a config file — things that hold facts but don't decide):

  > Does it make decisions, or just hold facts?

  Substrates hold facts; domains decide. If the user's answer pulls toward "decides" (or names the *people / system* deciding on the substrate), the real candidate is the deciding layer, not the substrate. Reflect that back and continue.

- **If the candidate sounds task-shaped** (a recurring activity, a periodic process — "code reviews every Friday," "weekly newsletter," "sprint planning"):

  > Is this a thing that recurs, or a thing with its own lifecycle?

  Recurring tasks belong to sub-agents — they're fired off per call, no persistence needed. Lifecycle = persistence territory. If the user's answer pulls toward "recurs," gently mark this candidate as sub-agent territory and continue. If the answer pulls toward "lifecycle" (state that compounds across the recurrences), the candidate is real.

- **If the candidate is vague** ("the codebase," "the project," "the business"):

  > What would break if no one owned it?

  Pulls the user toward the ownership consequence — what concretely degrades or rots without an owner? If the answer is concrete and load-bearing, the candidate is real. If the answer is vague, the candidate hasn't been named at the right altitude yet — ask the user to pick the most important piece of "the codebase" or "the project" and run the sharpening again.

The output of 2b is a refined candidate list: things that have been disambiguated from substrates, tasks, and vagueness.

### 2c. Stress-test the candidates

For each refined candidate, run **one** stress test. Don't run all three on the same candidate. Choose the one that fits.

- **The vacation-rot test** — when the candidate is real but the user hasn't articulated *why* it matters:

  > If no one owned this for two weeks, what would silently rot?

  Concrete answer ("examples drift, broken links accumulate, version mismatch creeps in") = the candidate holds. Vague answer ("nothing really, things just slow down") = the candidate may not be domain-shaped. Mark it and ask the user to reconsider.

- **The personification test** — when you want to surface whether the candidate has *positions*, not just state:

  > If [candidate] had opinions, what would they be?

  Domains have positions (the auth domain "wants" sessions short-lived; the docs domain "wants" examples runnable). If the user can name two or three opinions for the candidate, it's domain-shaped. If they can't, it's probably a substrate or a task.

- **The hiring test** — when the candidate is ambiguous between function and domain:

  > If you hired a human for this, would their title be a function (PM, QA, engineer) or a domain (auth, search, docs)?

  Function title = the cut is wrong; the candidate is shaped around what someone *does*, not what they *own*. Reflect that back: "Sounds like that's a function-shaped role. The work itself probably lives in one of the domains we've named — or it's a sub-agent." Then revisit the candidate at the domain altitude.

### 2d. Validate against the two gates

For candidates that survive sharpening and stress tests, confirm both gates explicitly with the user. This is conversational, not a checklist; you're confirming that the candidate is real.

**Gate (a) — entry conditions.** All three required:

- **Ownership.** Is this a *thing* the agent will own end-to-end — not a slice, not a task?
- **Time management.** Does the work have its own rhythm? Will the agent decide when to act, not just what to do when called?
- **Persistent memory.** Will context accrue — decisions today informing decisions next month?

**Gate (b) — at least one situational trigger.** Two parallel families. The user only needs one trigger total, but the families are categorically different — keep them separate.

*Work-shape triggers — about the work itself (apply to any candidate):*

- **Domain separation** (state, history, decisions that compound)
- **Focus separation** (continuous attention, not a fired-off task)

*Team-shape triggers — about a team of persistent agents (only meaningful when the user has 2+ candidates):*

- **Multiple opinions** (pushback from a peer with their own perspective and memory)
- **Synergy** (agents riff and improve each other's output over time)

If the user has surfaced only one candidate so far, **skip the team-shape triggers** — they're cross-agent properties, nonsensical for a one-agent team. The work-shape triggers alone are enough to validate a single-candidate gate.

If both gates pass, the candidate earns a persistent agent. If either gate fails, gently surface that to the user — "that one feels more sub-agent-shaped, let's set it aside" — and continue with the remaining candidates.

Stop the discovery loop when the user has surfaced **2-5 candidates that pass both gates**. Don't push for more; small teams are fine. If the user surfaces more than 5, ask whether to consolidate or to split into multiple projects (both are valid).

### 2e. Offer inspirations (optional, user-driven)

After the user has named their own domains, offer to show how other teams have cut theirs:

> Want to see how other teams have cut their domains? I can show you four shapes from teams that have run on teamctl — not to pick from, just to compare yours against.

If the user says yes, show the four legacy shapes (drawn from `examples/<folder>/.team/`) as a single inline reference, each with a one-line label describing what kind of work it fits:

> **OSS maintainer** — a maintainer + 4 workers (triage, bug-fix, docs, release-manager). Fits maintained open-source repos.
>
> **Editorial room** — an editor + 3 workers (writer, fact-checker, seo-research). Fits content publishing.
>
> **Indie studio** — a director + 3 workers (designer, writer, playtest-critic). Fits small game / product teams.
>
> **Solo triage** — a manager + 2 workers (research, inbox). Fits a single operator's working queue.

The user does **not** pick from these. They are shape-references only. The team you build in Stage 4 comes from the user's discovered domains, not from this list.

If the user passes on the inspirations, advance to Stage 3 directly.

### 2f. Fallback — user can't name anything

If, after both primary questions and a fair attempt at sharpening, the user cannot surface a single candidate domain, the honest surface is verbatim:

> Sometimes the cut isn't visible from the inside. You can hand-author `.team/team-compose.yaml` directly, or come back to `/teamctl:init` after you've worked with teamctl for a bit and the domains have surfaced themselves. Want me to point you at the docs page on how to think about teams?

If the user says yes, surface the link: [docs/src/content/docs/concepts/teams.md](../../../docs/src/content/docs/concepts/teams.md) (in the deployed docs, `/concepts/teams/`). Then exit gracefully — no scaffolding, no template fallback. The user will return when they're ready.

### 2g. Fallback — every candidate fails the gates

If the user names things but every candidate fails the gates (most often because the candidates are tasks, not domains), the honest surface is verbatim:

> The things you named are real, but they read more like tasks than domains. Tasks are sub-agent territory — they're already handled by the runtime, no persistence needed. Want to keep going and see if a domain surfaces, or pause here and read the docs page first?

Offer the docs link as an exit. If the user wants to keep going, return to **2a** with the second primary question (re-explanation cost) — different angle, different surface area.

If a third pass fails, treat it as the can't-name-anything fallback and exit gracefully. Don't force a wrong-shaped team into existence.

## Stage 3 — Propose org

Take the candidate domains the user surfaced in Stage 2 and propose an org. The team is named — never `team-1`, never `default`. Infer the name from the cwd's directory:

- `~/dev/acme-blog` → `Acme blog`
- `~/projects/sidequest-game` → `Sidequest game`

If the cwd name is generic (`workspace`, `project`, `dev`, `code`, `src`, single letter), prompt once for a name:

> What should I call this team?

If the user just hits enter, generate a sensible default from the surfaced domains (e.g., the first domain's name + " team": "Auth team," "Docs team"). Don't fall back to `team-1`.

### Synthesise the org

From the surfaced domains, suggest:

- **Manager** — the most operator-facing or coordination-shaped domain (the one the operator most often needs to talk to directly). If the user surfaced one obviously-coordination-shaped candidate, propose that. Otherwise, ask once: "Which of these domains do you want as the manager — the one you'll DM most often?"
- **Workers** — every other surfaced domain becomes a worker reporting to the manager.

If the user surfaced only **1 candidate**, propose a single-agent team (manager only, no workers). Single-agent teamctl teams are valid; the persistence and identity still earn the substrate.

If the user surfaced **more than 5**, ask whether to consolidate (some domains might compose into a single agent) or to split into multiple projects (each project gets its own manager + workers). Both are valid; don't silently truncate.

Render the org as a named ASCII tree. Manager on top with the "← you talk to this one on Telegram" annotation, workers fanning out below. Use the same shape the legacy stage used:

```
              ┌──────────────────┐
              │     <manager>    │ ← you talk to this one on Telegram
              └────────┬─────────┘
                       │
       ┌───────────────┼─────────────────┐
       │               │                 │
  ┌────▼────┐ ┌────────▼─────┐ ┌─────────▼────┐
  │ <wkr_1> │ │   <wkr_2>    │ │   <wkr_3>    │
  └─────────┘ └──────────────┘ └──────────────┘
```

Closing line:

```
N Claude Code agents · Opus 4.7 · effort high. ship it?
```

Where N is the surfaced count.

If the user confirms with "ship it", "yes", "go", or similar, advance to Stage 4. If they push back — wanting to rename an agent, swap a worker into the manager seat, drop a candidate — accept the edit inline and re-render the tree. The user can adjust freely here; they're confirming a synthesis, not picking from a menu. Once confirmed, advance.

If the user wants larger changes (different team name, redo the whole synthesis), step back to Stage 2 and re-sharpen. Don't force a shape the user doesn't recognise.

## Stage 4 — Scaffold from synthesis, then reveal

This is the moment the plugin commits to disk. Inputs handed off from Stages 2-3: the **surfaced domains** (each with the user's own words for what the agent owns + the gates that justified its persistence), the **manager / worker split**, the **team name**, and the **cwd** to scaffold into.

The plugin scaffolds `.team/` programmatically from the synthesised inputs. **No example folder is copied byte-for-byte.** The legacy `examples/<folder>/.team/` trees are role-prompt **substance inspiration** only (see role-prompt generation below); they are never the source of `team-compose.yaml` or `projects/<project-id>.yaml`.

### Derived inputs

- **Project id** — kebab-case slug of the team name. Lowercase, alphanumeric + hyphens, collapse runs of hyphens, trim leading/trailing hyphens. "Acme editorial" → `acme-editorial`. "Side-project triage!" → `side-project-triage`.
- **`tmux_prefix`** — `<project-id>-` (trailing hyphen). Used in `team-compose.yaml`.
- **Project-YAML filename** — `projects/<project-id>.yaml`.
- **Team display name** — the user's chosen string verbatim, written to the `name:` field in `projects/<project-id>.yaml`.

### Files to write

```
<cwd>/.team/
├── team-compose.yaml         # synthesised from a programmatic template; tmux_prefix + projects: file:
├── projects/<project-id>.yaml # synthesised — channels, manager, workers, Telegram interface placeholders
├── roles/<role>.md           # one per agent — generated on the fly, see below
├── .env.example              # canonical template; TEAMCTL_TG_<NAME>_TOKEN / _CHATS placeholders per manager
└── .gitignore                # canonical template
```

The shape of each file:

- **`team-compose.yaml`** — `version: 2`, broker `sqlite` at `state/mailbox.db`, supervisor `tmux` with `tmux_prefix: <project-id>-`, a single `projects:` entry pointing at `projects/<project-id>.yaml`, and a `globally_sensitive_actions` block carrying the canonical defaults (publish, release, deploy, payment, external messages — same shape the legacy examples use). No plugin-specific keys, no markers.
- **`projects/<project-id>.yaml`** — `project.id: <project-id>`, `project.name: <team display name>`, an `all` channel with `members: '*'`, a `managers:` map with the manager entry (runtime `claude-code`, model `claude-opus-4-7`, effort `high`, `role_prompt: roles/<manager>.md`, `interfaces.telegram` with `bot_token_env: TEAMCTL_TG_<MANAGER_UPPER>_TOKEN` and `chat_ids_env: TEAMCTL_TG_<MANAGER_UPPER>_CHATS`), and a `workers:` map with each worker entry (same fields except no `interfaces.telegram` block). `can_dm` and `can_broadcast` populated from the manager-worker relationships.
- **`.env.example`** — one block per manager, the two env vars commented with what to fill in. Same shape as legacy `.env.example` files.
- **`.gitignore`** — canonical: `state/`, `.env`.

**Substrate constraint #3 still applies**: the output must be byte-for-byte indistinguishable from a hand-authored team. No `# generated-by:` markers anywhere. No plugin-only keys. A user inspecting `team-compose.yaml` cannot tell it came from a plugin.

### Role-prompt generation

For each agent in `projects/<project-id>.yaml` — manager and each worker — generate `roles/<agent-id>.md` on the fly. Generation runs inside this Claude Code session: read the spine plus the role facts, then write the role prompt directly to disk.

For each agent, supply the model with:

1. **The 8-section spine**, read verbatim from `plugins/claude-code/role-prompt-style.md`. Every generated role prompt has all eight section headers in order: Identity, Mission, Voice, Best practices, Loop, Memory, Boundaries + HITL gates, Hard rules.
2. **Role facts** drawn from the synthesised inputs and the project YAML:
   - The agent's domain — the user's own words for what this agent owns (from Stage 2).
   - The gates that justified its persistence (which Gate (b) triggers fired from Stage 2d).
   - Agent kind (manager / worker), reports-to relationship, peers in the same project.
   - Channels the agent is on (`can_dm`, `can_broadcast` from the YAML).
   - HITL gates from the team's `globally_sensitive_actions`.
   - Telegram-bound or not (manager-only — read `interfaces.telegram` presence).
3. **Substance inspiration** — find the closest legacy `examples/<folder>/.team/roles/<role>.md` by domain shape (not by user-facing label). For an "auth" domain agent, look at how the example role prompts handle owning a specific surface end-to-end. **Read it for shape and tone, not for content copy.** Restate in the user's team's terms.
4. **Voice** — default coworker baseline (slack-style, short, concise, clear, emoji-friendly, "experienced reliable coworker"). Stage 6 regenerates Telegram-bound managers' prompts with custom-voice overrides if the user asks for one; Stage 4 doesn't pre-empt that.

Write the prompt directly to `<cwd>/.team/roles/<agent-id>.md`. No CLAUDE attribution in the file. No "generated by" footer. The prompt should read like a careful human wrote it.

### Validate

Run `teamctl validate` from `<cwd>`. Exit 0 is the gate.

If validate succeeds, advance to the reveal beat.

If validate fails:

> Hmm, validate flagged this: `<error verbatim>`. Want me to undo the `.team/` and stop, or leave it for you to inspect?

Surface the error **verbatim** — don't re-format, don't paraphrase, don't massage. The user gets the rollback choice or the inspect choice; honour either. Validation failure here means a plugin bug; the honest surface is the recovery path.

### Reveal beat

When validate is green, close Stage 4 with the literal text — substrate constraint #2, verbatim required:

> I wrote `.team/team-compose.yaml` for you — open it, everything we just talked about is in there.

Voice rails apply (1-2 sentences, "experienced reliable coworker"). Don't pad with a celebration paragraph; the line stands. Then advance to Stage 5.

## Stage 5 — Run

Single beat:

> Bring it up?

On confirm, run `teamctl up` from `<cwd>`. Parse the output for the agent count and the tmux-prefix-named sessions, then report inline:

```
✓ N sessions alive in tmux (<prefix><manager>, <prefix><wkr_1>, ...)
```

Adapt the count and the names from the synthesised roster and the project-id's `tmux_prefix`. The bullet is the whole beat — no celebration paragraph after it.

If `teamctl up` fails, surface the error verbatim and offer two paths forward — retry, or look at it together. Don't restart Stages 1-4. Voice rails: 1-2 sentences, "experienced reliable coworker," no apology spiral.

> `teamctl up` errored — here's what came back: `<error>`. Retry, or want to look at it together?

The "look at it together" beat is teammate-flavored on purpose. The user picked a path; if the runtime hiccupped, you're the colleague who debugs it with them, not the wrapper that gives up.

## Stage 6 — Telegram + voice-customize

The plugin **instructs**, doesn't wrap. `teamctl bot setup` is interactive (BotFather token paste, `/start` chat-id polling, env-var-name overrides) and runs in the user's terminal, not in a Bash subshell. Tell the user where to point the wizard; the wizard itself iterates managers, walks BotFather, captures token + chat id, writes `<cwd>/.team/.env`, and edits `interfaces.telegram` through the comment-preserving substrate (`team_core::yaml_edit`).

### Closure + defer

> All set up. Now let's connect your manager to Telegram.
>
> Run `teamctl bot setup` in a terminal — it'll walk you through it. (If anything breaks, run it again or skip and use `/teamctl:adjust` later.)

That's the whole defer. No re-explanation of BotFather, no token-capture preview, no env-var section. The wizard handles all of it.

The tail clause — *"If anything breaks, run it again or skip and use `/teamctl:adjust` later"* — is the substrate-constraint-#4 receipt at lighter weight. The wizard runs in the user's separate terminal, so the skill doesn't see exit codes; the one-line hint is the honest surface.

### Voice-customize sub-beat

Continue immediately after the defer beat — don't block-wait for the user to finish the wizard. Voice-customize is local config (it edits `roles/<manager>.md`), interface-independent, and the user can keep both threads moving. For the manager (only managers — workers stay on the Stage-4 default voice):

> Want to customize `<manager>`'s voice, or use the default?

**Default voice** (no regen): slack-style, short, concise, clear, emoji-friendly, proactive in sharing and checking with stakeholders, "experienced reliable coworker." Stage 4 already generated `roles/<manager>.md` with this voice; if the user picks default, you're done with voice for this manager.

**Custom voice** (triggers regen): ask the user to describe what they want, anchored on dimensions, not examples:

> Describe the voice you want — a sentence or two is plenty. Tone, formality, emoji use — whatever you want different.

Capture the overrides. Re-run the role-prompt-gen mechanism for THIS manager only, with the custom-voice override merged into section 3 (Voice) of the 8-section spine. Sections 1, 2, 4-8 stay as Stage 4 generated them. Overwrite `<cwd>/.team/roles/<manager>.md` with the regenerated prompt.

If the synthesised team has more than one manager (rare in v1 — only when the user explicitly surfaced multiple operator-facing domains and split into separate projects), drop the long default-voice description on subsequent prompts. *"Want to customize `<other-manager>`'s voice too, or use the default?"* is enough.

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

1. The plugin name on the marketplace card is **`teamctl`** — internal command names stay descriptive (`/teamctl:init`, `/teamctl:adjust`).
2. The reveal beat ("I wrote `.team/team-compose.yaml` for you…") fires at the end of Stage 4 — verbatim. Don't pre-empt it earlier; don't restyle it later.
3. The `.team/` output Stage 4 produces is byte-for-byte identical to a hand-authored team — no plugin-only state, no generated-by markers.
4. Every action this command takes is reproducible by hand-editing YAML afterwards.

## The principle this implements

The discovery conversation in Stage 2 teaches one thing: agents are cut by **domain ownership**, not by job function. PM / QA / engineer / designer reproduces a traditional org chart, which is often wrong even for humans. The right cut is by domains — things with their own state, history, and decisions that compound.

For the full methodology, the two-gate framing, anti-patterns, and rationale, read [`docs/src/content/docs/concepts/teams.md`](../../../docs/src/content/docs/concepts/teams.md). That page is the canonical companion. If anything in this skill drifts from the methodology there, the docs page wins.
