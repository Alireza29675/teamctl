---
description: Open-ended "talk to it" command for evolving an existing teamctl team — add a manager, add a worker, scope a channel, wire telegram, retire an agent.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl` is the command you reach for after onboarding ends. Describe the change in plain English — *"add a docs worker reporting to maintainer"*, *"scope the release channel to just maintainer and release_manager"*, *"retire the bug_fix worker"* — and `/teamctl` proposes the YAML diff, applies it on confirmation, validates, and offers to reload. No subcommands; the user just talks.

Read [RULES.md](../RULES.md) before each invocation. Substrate constraint #4 is the non-negotiable: every action this command takes is reproducible with `vim .team/team-compose.yaml`. No skill-only formats, no plugin-only state.

## Flow for any change

Six beats, in order. Each beat is 1-2 sentences in your voice:

1. **Read.** Open `.team/team-compose.yaml` and the relevant `projects/<id>.yaml`. If the team has more than one project, ask which.
2. **Propose.** Describe the change in plain English — sound like a teammate explaining what you'd type — then show the unified YAML diff (3 lines context). For diffs that touch multiple non-adjacent sections, narrate which sections will be touched *before* showing the diff.
3. **Confirm.** Short prompt — `Apply?` or `Ship it?` Not *"Do you want me to proceed with the changes outlined above?"*
4. **Apply.** Edit the YAML with targeted `Edit` calls so comments and blank lines outside the edit survive. For a new role prompt, write `roles/<name>.md` per the [8-section spine](../role-prompt-style.md).
5. **Validate.** Run `teamctl validate`. If it fails, surface the error verbatim and offer to roll back.
6. **Offer reload.** *"Your team is updated. Reload to apply: `teamctl reload`?"* Run with confirmation if the user says yes.

## Voice rails

The propose step is the load-bearing surface. The user reads it, screenshots it, posts it. Get this right.

- 1-2 sentences per beat. No walls.
- Teammate, not linter. Three failure modes to avoid:
  - **Action-shape narration.** *"Action: insert worker entry, key=docs, parent=maintainer"* — robot announcing operations.
  - **Imperative-mood narration of YAML structure.** *"Modify the `members:` list to include the new agent"* — tutorial, not teammate.
  - **Passive-voice schema speech.** *"A new entry will be created under `workers:` referencing the existing manager"* — docs, not conversation.
  Lift to the teammate variant: *"I'll add a `docs` worker reporting to `maintainer`, same Sonnet-on-low-risk-only profile as the others, and add `docs` to maintainer's `can_dm`."* Real product nouns, backticks for identifiers, `list` not `members array`.
- The YAML diff is the receipt. Unified diff with 3 lines context. Renders cleanly in markdown and plaintext (matters when the user screenshots it).
- Body voice is runtime-neutral. *"Claude Code runtime"* is a fact about the agent and stays; *"Claude reads the file"* is voice drift and goes.
- Confirmation prompts are short. `Apply?` is enough.

### Multi-hunk narration

When a verb's diff touches more than one non-adjacent section, name the sections before showing the diff. Example for verb 5 (retire `bug_fix`):

> Retiring `bug_fix` touches three places — the `workers:` entry, the `dev` channel's `members:` list, and `maintainer`'s `can_dm`. Here's the diff:

A teammate tells you what's coming; the diff is the receipt.

## The five v1 verbs

### Verb 1 — Add a manager

User says: *"add a release_manager"*, *"add a manager named ops_lead"*, *"give me a manager that handles partner emails"*.

Touches:
- A new entry under `managers:` in the project YAML — `runtime: claude-code`, `model: claude-opus-4-7`, `role_prompt: roles/<name>.md`, `permission_mode: auto`, `autonomy: low_risk_only`, `can_dm: []`, `can_broadcast: [all]`.
- A new `roles/<name>.md` written per the [8-section spine](../role-prompt-style.md).
- If the user mentions telegram (*"with telegram"*, *"for me to reach"*), inline an `interfaces.telegram` block with `bot_token_env: TEAMCTL_TG_<NAME>_TOKEN` / `chat_ids_env: TEAMCTL_TG_<NAME>_CHATS` (canonical pattern), and tell the user to run `teamctl bot setup` afterwards to register the actual bot.
- If a named channel should include the new manager (the user might say so, or it's obvious from context — e.g. an `all` channel), update its `members:` list. If unclear, ask.

Propose voice example:

> I'll add a `release_manager` manager — Claude Code on Opus, plan-mode-friendly autonomy, with an empty `can_dm` ready for you to fill in. I'll also write `roles/release_manager.md` with the 8-section spine. Want telegram on it now or later?

### Verb 2 — Add a worker

User says: *"add a docs worker"*, *"add a researcher reporting to the editor"*.

Touches:
- A new entry under `workers:` — `runtime: claude-code`, `model: claude-sonnet-4-6` (cost-tier-appropriate default), `permission_mode: auto`, `reports_to: <manager>`, `can_dm: [<manager>]`, `can_broadcast: []`.
- A new `roles/<name>.md` per the [8-section spine](../role-prompt-style.md).
- The worker added to the manager's `can_dm` list (so the manager can route to it). If multiple managers exist, ask which.
- Pipeline channel update (e.g. if a `dev` channel exists and the new worker fits the pipeline, add the worker there). If ambiguous, ask.

Propose voice example:

> I'll add a `docs` worker reporting to `maintainer`, same Sonnet-on-low-risk-only profile as the other workers, with `can_dm: [maintainer]`. I'll also add `docs` to maintainer's `can_dm` so they can route to it, and add `docs` to the `all` channel for end-of-day broadcasts. New `roles/docs.md` lands with the spine pre-filled.

### Verb 3 — Scope a channel

User says: *"make the release channel only the maintainer and release_manager"*, *"drop docs from the dev channel"*, *"let the new worker post to all"*.

Touches:
- The channel's `members:` list.
- If the change removes a member who had `can_broadcast: [<channel>]`, surface that — *"`docs` had `can_broadcast: [dev]`; should I remove that too?"* — and ask. Don't silently revoke broadcast rights.

Propose voice example:

> Scoping `release` to just `maintainer` and `release_manager` — dropping `docs` and `triage` from the channel. Neither has `can_broadcast: [release]` so no permission cleanup needed.

### Verb 4 — Wire telegram on an existing manager

User says: *"wire telegram on maintainer"*, *"give the editor telegram access"*.

Touches:
- An `interfaces.telegram` block on the manager entry: `bot_token_env: TEAMCTL_TG_<NAME>_TOKEN` / `chat_ids_env: TEAMCTL_TG_<NAME>_CHATS`.
- Matching entries seeded in `.team/.env.example` (canonical `TEAMCTL_TG_<NAME>_*` shape).
- After applying the YAML edit + `teamctl validate` exits 0, run `teamctl bot setup` for that manager. The wizard walks BotFather → token → `/start` → chat id and writes the values into `.team/.env`. Same wrap as Stage 6 of `/teamctl-init`.

Propose voice example:

> Wiring telegram on `maintainer`. I'll add an `interfaces.telegram` block referencing `TEAMCTL_TG_MAINTAINER_TOKEN` and `TEAMCTL_TG_MAINTAINER_CHATS`, seed those names in `.team/.env.example`, then walk you through `teamctl bot setup` so the actual bot is registered.

#### Heads-up if telegram is already wired

If `interfaces.telegram` already exists on the manager and the user is asking for a re-wire (or running `bot setup --force`), warn before applying:

> Heads-up — `maintainer` already has telegram wired. Re-wiring overwrites the `interfaces.telegram` block; any inline comments inside it will be lost (comments around it survive). Continue?

Plain warning in your voice; not a stack trace, not a corporate disclaimer.

### Verb 5 — Retire an agent

User says: *"retire the bug_fix worker"*, *"remove ops_lead"*, *"drop the docs role"*.

Touches:
- The agent's entry under `managers:` or `workers:` — removed wholesale.
- The agent's name removed from any channel's `members:` list.
- Any `reports_to: <retired>` references on other workers — ask whether to re-route to another manager or leave the worker without one.
- Any `can_dm: [<retired>]` references on other agents — removed.
- The `roles/<retired>.md` file — default is keep (the user might want to repurpose it); offer to delete.

This is a multi-hunk verb. Narrate the sections first:

> Retiring `bug_fix` touches three places — the `workers:` entry, the `dev` channel's `members:` list (`bug_fix` shares it with `maintainer`), and `maintainer`'s `can_dm`. The `roles/bug_fix.md` file stays unless you want it gone. Here's the diff:

#### Heads-up — comments inside the retired section go with it

Removing an agent's section also removes any inline comments inside it (comments above and below the section, and elsewhere in the file, survive). Warn before applying:

> Heads-up — removing `bug_fix`'s section drops any inline comments you wrote inside it. Surrounding comments are safe. Continue?

## Apply mechanics — keep edits surgical

Use `Edit` (targeted edits), not `Write` (full-file rewrite), for every YAML mutation. Targeted edits leave every line you didn't touch byte-identical, which is what keeps the file looking hand-authored. Full-file rewrites reflow comments and lose blank-line clusters even when the data is the same.

For verbs 1, 2, 3, 5 the agent edits the YAML directly with `Edit`. Verb 4's wire-telegram step also uses `Edit` for the YAML; the `teamctl bot setup` wrap is what registers the bot with BotFather and writes the chat id into `.team/.env`.

What the substrate guarantees (since `team-core::yaml_edit`):
- Comments **between** top-level YAML blocks survive every operation.
- Blank-line clusters survive.
- Key ordering survives.
- Comments **inside** a wholesale-replaced or removed block are dropped — verb 4 wholesale rewrites the `telegram:` block; verb 5 wholesale removes the agent's section. The user gets a heads-up before either applies.
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

- **Verbs beyond the five named.** *Change a model*, *swap runtime*, *rename an agent*, *split a project into two* — surface as: *"v1 of `/teamctl` covers add/scope/wire/retire on agents and channels. For \<what they asked for\>, the cleanest path is `vim .team/team-compose.yaml` — happy to walk you through the change you want to make."*
- **Multi-project edits in one go.** v1 handles one project at a time. If the team has multiple `projects/<id>.yaml`, ask which.
- **Bulk operations.** *"Add 3 workers"* works (handled in sequence with confirm-each); *"bulk swap all workers from sonnet to opus"* is out of v1.
- **Undo / replay history.** v1 is forward-only. Point at `git diff` for the receipt.

## Reviewer test

After any verb completes, hand the resulting `.team/` to someone unfamiliar with the plugin and ask: *would you have known a tool wrote this?* The answer should be no.
