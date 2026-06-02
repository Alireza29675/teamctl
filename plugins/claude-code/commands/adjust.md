---
description: Open-ended "talk to it" command for evolving an existing teamctl team — hire or retire an agent, modify its behaviour or capability stack (sub-agents / skills / `/loop` / hook), scope a channel, set up Telegram, add a project, or open a bridge.
allowed-tools: Bash, Read, Write, Edit, AskUserQuestion
---

`/teamctl:adjust` is the command you reach for after onboarding ends. Describe the change in plain English — *"add a docs worker reporting to maintainer"*, *"retire the bug_fix worker"*, *"open a bridge to the ops project"* — and `/teamctl:adjust` walks you through the picks, proposes the YAML diff, applies it on confirmation, validates, and offers to reload.

Read [RULES.md](../RULES.md), [INTERACTIVE.md](../INTERACTIVE.md), and [capability-catalog.md](../capability-catalog.md) before each invocation. RULES carries the architecture invariants (including the capability invariants — capabilities-over-seats, no-cron/no-extra-MCP in the team); INTERACTIVE carries the UI invariants (when to reach for `AskUserQuestion`, the Apply/Modify/Reject gate, the headless-pane fallback, docs-as-ground-truth, voice control); the catalog is the palette for the capability stack a hired agent earns and Verb 8 evolves. Substrate constraint #4 is the non-negotiable: every action this command takes is reproducible with `vim .team/team-compose.yaml`. The mindset has to survive *evolution*, not just init — a hired agent gets its capability stack, and the stack is a first-class thing to adjust.

## Preamble — detect interactive vs headless

At the top of every invocation, decide which mode you're in:

```bash
if [ -n "$TEAMCTL_ROOT" ] && [ -n "$AGENT_ID" ]; then
    echo "headless"
else
    echo "interactive"
fi
```

`headless` means a supervised teamctl agent (env-file rendered by `teamctl render`, sourced by `agent-wrapper.sh`) is calling this skill — `AskUserQuestion` is denied by the wrapper's `PreToolUse` hook (see [#189](https://github.com/Alireza29675/teamctl/issues/189)). Fall back to plain-text Q&A for the whole invocation per [INTERACTIVE.md §5](../INTERACTIVE.md). `interactive` means a normal user `claude` session — use `AskUserQuestion` everywhere a finite-set pick fits.

## Action picker — at the top of every invocation

If the user opens with a clear intent (*"add a docs worker"*, *"retire the bug_fix role"*, *"wire telegram on maintainer"*, *"open a bridge to ops"*), skip the picker and jump straight to the matching verb section below. If the intent is ambiguous or the user wants help deciding, open with:

```text
question: "What do you want to adjust?"
header: "Adjust"
options:
  - label: "Add or remove an agent"
    description: "Hire a manager / worker, retire one, or move where they report."
  - label: "Modify an agent"
    description: "Change behaviour (role prompt, model, autonomy) — or its capability stack (sub-agents, skills, `/loop`, hook)."
  - label: "Adjust a channel or bridge"
    description: "Scope channel members, broadcast rights, or open a cross-project bridge."
  - label: "Add a new project"
    description: "Scaffold a new `projects/<id>.yaml` and wire it into compose."
```

Each branch narrows further with `AskUserQuestion` as needed (e.g. *manager or worker?*, *which agent?*, *which channel?*). `Modify an agent` narrows to **behaviour** (Verb 5) or **capability stack** (Verb 8). A clear intent skips the picker: *"give `builder` a `qa-tester` sub-agent"* jumps straight to Verb 8, *"swap maintainer to opus"* to Verb 5.

## Universal flow for any state-mutating change

Six beats, in order. Each beat is 1-2 sentences in your voice:

1. **Read.** Open `.team/team-compose.yaml` and the relevant `projects/<id>.yaml`. If the team has more than one project, pick which with `AskUserQuestion` (or prose-list in headless).
2. **Propose.** Describe the change in plain English — sound like a teammate explaining what you'd type — then show the unified YAML diff (3 lines context). For diffs that touch multiple non-adjacent sections, narrate which sections will be touched *before* showing the diff. For team-shape proposals, cite the relevant `docs/src/content/docs/concepts/*.md` page (see [INTERACTIVE.md §6](../INTERACTIVE.md)).
3. **Gate.** Fire the Apply/Modify/Reject picker — canonical shape in [INTERACTIVE.md §4](../INTERACTIVE.md). Tight; no recap.
4. **Apply.** On `Apply`, edit the YAML with targeted `Edit` calls so comments and blank lines outside the edit survive. For a new role prompt, write `roles/<name>.md` per the [8-section spine](../role-prompt-style.md). On `Modify`, ask one cutting follow-up question and re-propose. On `Reject`, write nothing and exit.
5. **Validate.** Run `teamctl validate`. If it fails, surface the error verbatim and offer to roll back.
6. **Offer reload.** *"Your team is updated. Reload to apply: `teamctl reload`?"* Single yes/no — fire as `AskUserQuestion` in interactive mode, prose prompt in headless.

## Voice rails

The propose step is the load-bearing surface. The user reads it, screenshots it, posts it. Get this right.

- 1-2 sentences per beat. No walls.
- Teammate, not linter. Three failure modes to avoid:
  - **Action-shape narration.** *"Action: insert worker entry, key=docs, parent=maintainer"* — robot announcing operations.
  - **Imperative-mood narration of YAML structure.** *"Modify the `members:` list to include the new agent"* — tutorial, not teammate.
  - **Passive-voice schema speech.** *"A new entry will be created under `workers:` referencing the existing manager"* — docs, not conversation. Lift to the teammate variant: *"I'll add a `docs` worker reporting to `maintainer`, same Sonnet-on-low-risk-only profile as the others, and add `docs` to maintainer's `can_dm`."* Real product nouns, backticks for identifiers, `list` not `members array`.
- The YAML diff is the receipt. Unified diff with 3 lines context. Renders cleanly in markdown and plaintext (matters when the user screenshots it).
- Body voice is runtime-neutral. *"Claude Code runtime"* is a fact about the agent and stays; *"Claude reads the file"* is voice drift and goes.
- The closing beat is the **Apply/Modify/Reject gate** — canonical shape in [INTERACTIVE.md §4](../INTERACTIVE.md). Tight, no recap. In headless mode the gate becomes prose: *"Apply this change? Reply `apply`, `modify` (and say what to adjust), or `reject`."*

### Multi-hunk narration

When a verb's diff touches more than one non-adjacent section, name the sections before showing the diff. Example for Verb 4 (retire `bug_fix`):

> Retiring `bug_fix` touches three places — the `workers:` entry, the `dev` channel's `members:` list, and `maintainer`'s `can_dm`. Here's the diff:

A teammate tells you what's coming; the diff is the receipt.

## v1 verbs

Each verb gathers its inputs via `AskUserQuestion` (in interactive mode) or numbered plain-text prompts (in headless), proposes the change with cited reasoning, then fires the Apply/Modify/Reject gate.

### Verb 1 — Hire an agent (manager or worker)

User says: *"add a release_manager"*, *"add a docs worker"*, *"give me a manager that handles partner emails"*. If the user's intent is ambiguous between manager and worker, fire `AskUserQuestion`:

```text
question: "Manager or worker?"
header: "Kind"
options:
  - label: "Manager"
    description: "Talks to you on Telegram; has `can_broadcast: [all]`."
  - label: "Worker"
    description: "Reports to a manager; no Telegram, no broadcast."
```

#### Verb 1a — Add a manager

Touches:
- A new entry under `managers:` in the project YAML — `runtime: claude-code`, `model: claude-opus-4-8`, `role_prompt: roles/<name>.md`, `permission_mode: auto`, `autonomy: low_risk_only`, `can_dm: []`, `can_broadcast: [all]`.
- A new `roles/<name>.md` written per the [8-section spine](../role-prompt-style.md).
- The agent's **capability stack** from [capability-catalog.md](../capability-catalog.md), emitted the same way `/teamctl:init` Stage 4 does (materialize the adapted files, declare the keys, file-first). A new manager is usually compass-shaped — name its research/forwarding sub-agents (`pr-summarizer` for a manager who forwards PRs, the research set for an ideation-shaped one), any skills, and that it runs **no `/loop`** (managers are conversational). The propose beat names the stack alongside reports-to.
- If the user mentions telegram (*"with telegram"*, *"for me to reach"*), inline an `interfaces.telegram` block with `bot_token_env: TEAMCTL_TG_<NAME>_TOKEN` / `chat_ids_env: TEAMCTL_TG_<NAME>_CHATS` (canonical pattern), and tell the user to run `teamctl bot setup` afterwards to register the actual bot.
- If a named channel should include the new manager (the user might say so, or it's obvious from context — e.g. an `all` channel), update its `members:` list. If unclear, ask.

Propose voice example (cite `concepts/teams.md` for the reports-to rationale, `concepts/interfaces.md` if telegram comes up):

> I'll add a `release_manager` manager — Claude Code on Opus, plan-mode-friendly autonomy, with an empty `can_dm` ready for you to fill in. I'll also write `roles/release_manager.md` with the 8-section spine. Per [teams.md](../../../docs/src/content/docs/concepts/teams.md), this agent earns its slot by owning the release domain end-to-end (clears the ship-alone test).

Then ask the inline telegram pick:

```text
question: "Wire Telegram on `release_manager` now?"
header: "Telegram"
options:
  - label: "Now"
    description: "Inline an `interfaces.telegram` block; you'll run `teamctl bot setup` after Apply."
  - label: "Later"
    description: "Skip — you can wire it later with `Set up Telegram` on `/teamctl:adjust`."
```

Then fire the Apply/Modify/Reject gate.

#### Verb 1b — Add a worker

User says: *"add a docs worker"*, *"add a researcher reporting to the editor"*.

**Capabilities-not-seats — check before proposing a seat.** If the ask reads like *more capability for an existing agent* rather than a new domain (*"add a QA worker"* when an engineer already owns the code, *"add someone to write release notes"* when a maintainer ships releases), surface the leaner path first. Resolve which existing agent it lands on from the ask (ask in one line if it's not obvious), then fire `AskUserQuestion` with `<agent>` filled in:

```text
question: "That sounds like a capability on `<agent>`, not a new agent — leaner. Which?"
header: "Seat?"
options:
  - label: "Capability on `<agent>`"
    description: "Add it as a sub-agent/skill on the existing agent (Verb 8). No new seat."
  - label: "New worker"
    description: "It's a real new domain that earns its own agent — proceed with the hire."
```

On `Capability on <agent>`, route to **Verb 8**. On `New worker`, continue. A new agent is earned only by a new **domain** (RULES: capabilities-over-seats) — proceed only once it clears that bar.

Touches:
- A new entry under `workers:` — `runtime: claude-code`, `model: claude-sonnet-4-6` (cost-tier-appropriate default), `permission_mode: auto`, `reports_to: <manager>`, `can_dm: [<manager>]`, `can_broadcast: []`.
- A new `roles/<name>.md` per the [8-section spine](../role-prompt-style.md).
- The worker's **capability stack** from [capability-catalog.md](../capability-catalog.md), emitted like `/teamctl:init` Stage 4 (materialize adapted files, declare keys, file-first). A builder-shaped worker earns the build-side sub-agents + `ship-it`/`tdd` + the `fmt-lint` hook (iff it writes code) + a `/loop`; a research-shaped worker earns the research set + `shape-idea`, no `/loop`. The propose beat names the stack.
- The worker added to the manager's `can_dm` list (so the manager can route to it).
- Pipeline channel update (e.g. if a `dev` channel exists and the new worker fits the pipeline, add the worker there). If ambiguous, ask.

If multiple managers exist and the user hasn't named one, pick with `AskUserQuestion` (one option per manager, ≤4; if more than 4, ask in prose). For the new worker's domain reasoning, apply the **ship-alone test** in the propose beat: can this worker ship its artifact alone? If no, surface that the cut is function-shaped and propose a domain-shaped alternative before the gate.

Propose voice example:

> I'll add a `docs` worker reporting to `maintainer`, same Sonnet-on-low-risk-only profile as the other workers, with `can_dm: [maintainer]`. I'll also add `docs` to maintainer's `can_dm` so they can route to it, and add `docs` to the `all` channel for end-of-day broadcasts. Per [teams.md](../../../docs/src/content/docs/concepts/teams.md), `docs` clears the ship-alone test — it owns the docs surface end-to-end, consuming auth/billing/whatever as substrate. New `roles/docs.md` lands with the spine pre-filled.

### Verb 2 — Scope a channel

User says: *"make the release channel only the maintainer and release_manager"*, *"drop docs from the dev channel"*, *"let the new worker post to all"*.

Touches:
- The channel's `members:` list.
- If the change removes a member who had `can_broadcast: [<channel>]`, surface that and fire `AskUserQuestion` — *"`docs` had `can_broadcast: [dev]`; revoke that too?"* with options `Revoke` / `Keep`. Don't silently revoke broadcast rights. Cite [channels.md](../../../docs/src/content/docs/concepts/channels.md) in the propose beat.

Propose voice example:

> Scoping `release` to just `maintainer` and `release_manager` — dropping `docs` and `triage` from the channel. Neither has `can_broadcast: [release]` so no permission cleanup needed.

### Verb 3 — Set up Telegram on an existing manager

User says: *"wire telegram on maintainer"*, *"give the editor telegram access"*.

Touches:
- An `interfaces.telegram` block on the manager entry: `bot_token_env: TEAMCTL_TG_<NAME>_TOKEN` / `chat_ids_env: TEAMCTL_TG_<NAME>_CHATS`.
- Matching entries seeded in `.team/.env.example` (canonical `TEAMCTL_TG_<NAME>_*` shape).
- After applying the YAML edit + `teamctl validate` exits 0, run `teamctl bot setup` for that manager. The wizard walks BotFather → token → `/start` → chat id and writes the values into `.team/.env`. Same wrap as Stage 6 of `/teamctl:init`.

Propose voice example:

> Wiring telegram on `maintainer`. I'll add an `interfaces.telegram` block referencing `TEAMCTL_TG_MAINTAINER_TOKEN` and `TEAMCTL_TG_MAINTAINER_CHATS`, seed those names in `.team/.env.example`, then walk you through `teamctl bot setup` so the actual bot is registered.

#### Heads-up if telegram is already wired

If `interfaces.telegram` already exists on the manager and the user is asking for a re-wire (or running `bot setup --force`), surface the side-effect in the propose beat — *"Re-wiring overwrites the `interfaces.telegram` block; any inline comments inside it will be lost (comments around it survive)."* — then fire the Apply/Modify/Reject gate. No second confirmation: the gate is enough.

### Verb 4 — Retire an agent

User says: *"retire the bug_fix worker"*, *"remove ops_lead"*, *"drop the docs role"*.

Touches:
- The agent's entry under `managers:` or `workers:` — removed wholesale.
- The agent's name removed from any channel's `members:` list.
- Any `reports_to: <retired>` references on other workers — fire `AskUserQuestion` to pick a re-route target (one option per remaining manager, plus a `Leave unowned` option) before proposing.
- Any `can_dm: [<retired>]` references on other agents — removed.
- The `roles/<retired>.md` file — pick with `AskUserQuestion`: `Keep` (default, repurpose later) or `Delete`.

This is a multi-hunk verb. Narrate the sections first:

> Retiring `bug_fix` touches three places — the `workers:` entry, the `dev` channel's `members:` list (`bug_fix` shares it with `maintainer`), and `maintainer`'s `can_dm`. The `roles/bug_fix.md` file stays unless you want it gone. Here's the diff:

#### Heads-up — comments inside the retired section go with it

Surface in the propose beat — *"Removing `bug_fix`'s section drops any inline comments you wrote inside it. Surrounding comments are safe."* — then the Apply/Modify/Reject gate carries the decision.

### Verb 5 — Modify an agent's behaviour

User says: *"swap maintainer to opus"*, *"change docs's autonomy to full"*, *"give triage plan-mode access"*, *"rewrite the editor's role prompt"*.

Touches one or more of:
- `model` — pick from runtime-supported list with `AskUserQuestion`.
- `runtime` — pick from `claude-code` / `codex` / `gemini` with `AskUserQuestion`; cite [runtimes.md](../../../docs/src/content/docs/concepts/runtimes.md).
- `autonomy` — pick `full` / `low_risk_only` / `proposal_only` with `AskUserQuestion`; cite [hitl.md](../../../docs/src/content/docs/concepts/hitl.md).
- `permission_mode` — pick `auto` / `attended` (when [#189](https://github.com/Alireza29675/teamctl/issues/189) lands) with `AskUserQuestion`.
- `role_prompt` — open the existing `roles/<agent>.md` for in-place edit; the user describes the change in free-form and the skill proposes a unified diff against the file.
- **capability stack** — add/remove a sub-agent, add/remove a skill, toggle the `/loop` drive, add/remove the `fmt-lint` hook. This is **Verb 8**; route there for the catalog pick + materialize-and-emit mechanics rather than hand-editing the keys here.

If the user names multiple dimensions in one breath, walk them in order, one gate per dimension (a multi-dimension batch can land as a single gate if the diff is small and coherent — judgment call).

Propose voice example (model swap, single dimension):

> Swapping `maintainer` from `claude-sonnet-4-6` to `claude-opus-4-8`. Per [runtimes.md](../../../docs/src/content/docs/concepts/runtimes.md), Opus is the default for managers — stronger on planning + tool use. Cost goes up; that's the trade.

### Verb 6 — Open a bridge between two projects

User says: *"open a bridge between `teamctl` and `ops`"*, *"let `release_manager` DM the `ops:otto` agent"*.

Touches:
- A `bridges:` entry in the global `team-compose.yaml` naming the two projects and the agents allowed to cross. Cite [bridges.md](../../../docs/src/content/docs/concepts/bridges.md).
- The receiving project's `can_dm` ACL extended to allow the source agent (only if the user's intent makes that explicit; otherwise the bridge is one-way).

Pick the allowlist with `AskUserQuestion` (one option per candidate agent per side, ≤4 each; if more than 4, ask in prose). For each named agent, confirm direction (source-only, target-only, both) with a follow-up `AskUserQuestion`. Then propose with a tree-style diagram of which agents can DM which (`source:agent → target:agent`), then the YAML diff, then the gate.

Propose voice example:

> Opening a bridge between `teamctl` and `ops`. `teamctl:release_manager` will be able to DM `ops:otto`; one-way, no reverse. Per [bridges.md](../../../docs/src/content/docs/concepts/bridges.md), bridges are explicit allowlists — no transitive trust, no implicit broadcast routing.

### Verb 7 — Add a new project

User says: *"add a new project `ops`"*, *"scaffold a partner project"*.

Touches:
- A new `projects/<id>.yaml` skeleton — `project.id`, `project.name`, an `all` channel, and one starter manager (the user names them).
- The global `team-compose.yaml` `projects:` map gains a `file:` entry pointing at the new file.

Walk the project shape via `AskUserQuestion` series — name (free-form), starter manager kind (`Manager` / `No starter, scaffold empty`), starter manager name (free-form). Cite [projects.md](../../../docs/src/content/docs/concepts/projects.md). Propose, gate, write, validate, reload.

Propose voice example:

> Scaffolding a new project — id `ops`, name `Ops`. One starter manager `otto`, Claude Code on Opus, on the `all` channel. The global `team-compose.yaml` picks up a new `projects:` entry pointing at `projects/ops.yaml`. Per [projects.md](../../../docs/src/content/docs/concepts/projects.md), projects are isolated by default — agents in `ops` can only DM `teamctl` agents if you also open a bridge (Verb 6).

### Verb 8 — Adjust an agent's capability stack

User says: *"give `builder` a `qa-tester` sub-agent"*, *"add the `tdd` skill to `eng`"*, *"turn on the goal→ship loop for `builder`"*, *"drop the `code-roaster` sub-agent from `docs`"*, *"add the fmt-lint hook to `eng`"*. This is how the capability mindset survives evolution — the stack is a first-class thing to adjust, picked from the same [capability-catalog.md](../capability-catalog.md) `/teamctl:init` emits from.

If the agent or the capability isn't named, narrow with `AskUserQuestion` (which agent? · add or remove? · sub-agent / skill / `/loop` / hook? · which one from the catalog?).

Touches — by capability kind:

- **Sub-agent (add).** Materialize `subagents/<name>.md` adapted to the agent's domain from the catalog (skip if the file already exists — a sub-agent shared across agents is one file), **then** add its path to the agent's `subagents:` list (file-first). 
- **Sub-agent (remove).** Drop the path from `subagents:`. The `subagents/<name>.md` file — pick `Keep` (default; another agent may use it, or you'll re-add it) or `Delete` (only if no other agent declares it), same as Verb 4's role-file choice.
- **Skill (add / remove).** Add: materialize `skills/<name>/SKILL.md` from the catalog, then add `skills/<name>` (the **dir** path) to `skills:`. Remove: drop the path; `Keep`/`Delete` the dir.
- **Hook (add / remove).** Add: materialize `hooks/fmt-lint.sh` (`chmod +x`), then add the `{event: PreToolUse, matcher: "Edit|Write", command: hooks/fmt-lint.sh}` object to `hooks:`. Only `fmt-lint` is earned (RULES). Remove: drop the object; `Keep`/`Delete` the script.
- **`/loop` (toggle).** `/loop` is **not a YAML key** — it's a role-prompt behaviour. Toggling it edits the agent's `roles/<agent>.md` **§5 Loop**: turning it *on* makes the agent builder-shaped (enter `/loop` dynamic mode on a handed goal → build/test/fix → PR + `request_approval`); turning it *off* reverts §5 to the plain idle loop. Propose the §5 diff like any `role_prompt` edit (Verb 5). Turning it *on* usually means also adding the **`ship-it`** skill so the builder has the drive to run — offer that as a second capability edit in the same gate (don't leave a builder-shaped §5 with no `ship-it` declared; that's the prompt/YAML desync `role-prompt-style.md` warns against). No `mcps:` and no cron are ever added — `/loop` is the heartbeat (RULES).

**Self-check after emit** (mirrors init Stage 4): `teamctl validate` confirms schema + coherence but does **not** check that the new path exists — confirm each added `subagents:`/`skills:`/`hooks:` path resolves on disk before validate, since a dangling one validates green and only fails (for sub-agents) at `teamctl reload`.

Propose voice example:

> Adding a `qa-tester` sub-agent to `builder`. I'll write `subagents/qa-tester.md` adapted to your stack, then add it to `builder`'s `subagents:` list. Per the capability mindset, this is a capability on an agent you already have — leaner than a new QA seat. Reload picks it up.

Then fire the Apply/Modify/Reject gate, validate, and offer reload like every other verb.

## Apply mechanics — keep edits surgical

Use `Edit` (targeted edits), not `Write` (full-file rewrite), for every YAML mutation. Targeted edits leave every line you didn't touch byte-identical, which is what keeps the file looking hand-authored. Full-file rewrites reflow comments and lose blank-line clusters even when the data is the same.

Every verb edits the YAML directly with `Edit`. Verb 3's Telegram setup also wraps `teamctl bot setup` after the YAML edit lands — that wizard registers the bot with BotFather and writes the chat id into `.team/.env`.

What the substrate guarantees (since `team-core::yaml_edit`):
- Comments **between** top-level YAML blocks survive every operation.
- Blank-line clusters survive.
- Key ordering survives.
- Comments **inside** a wholesale-replaced or removed block are dropped — Verb 3 wholesale rewrites the `telegram:` block on re-wire; Verb 4 wholesale removes the agent's section. The propose beat surfaces the side-effect before the gate.
- Round-trip on unchanged YAML is byte-perfect.

## Validate, then reload

After every apply, run:

```bash
teamctl validate
```

Exit 0 means the schema, ACLs, and project-isolation invariants all hold. If it fails, paste the error verbatim and offer to roll back the edit (the user's last `git diff` is the source of truth — point them at it).

If validate exits 0, offer the reload:

> Your team is updated. Reload to apply: `teamctl reload`?

Wait for confirmation. `teamctl reload` restarts only the agents whose config actually changed — no full teardown, no lost mailbox state.

## Out of scope (v1)

- **Verbs beyond the eight named.** *Rename an agent*, *split a project into two*, *merge two projects* — surface as: *"v1 of `/teamctl:adjust` covers hire / scope / Telegram / retire / modify / bridge / add-project / adjust-capabilities. For \<what they asked for\>, the cleanest path is `vim .team/team-compose.yaml` — happy to walk you through the change you want to make."*
- **Multi-project edits in one go.** v1 handles one project at a time. If the team has multiple `projects/<id>.yaml`, pick which with `AskUserQuestion`.
- **Bulk operations.** *"Add 3 workers"* works (handled in sequence with one gate each); *"bulk swap all workers from sonnet to opus"* is out of v1.
- **Undo / replay history.** v1 is forward-only. Point at `git diff` for the receipt; `Reject` at the gate never writes anything in the first place.

## Reviewer test

After any verb completes, hand the resulting `.team/` to someone unfamiliar with the plugin and ask: *would you have known a tool wrote this?* The answer should be no.
