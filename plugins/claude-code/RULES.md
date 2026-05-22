# RULES

Shared guardrails every command in this plugin reads off. Both `/teamctl:init` and `/teamctl:adjust` honour every rule below; reviewers check against this file.

## Voice rails

- 1-2 sentences per beat. No walls of text.
- Tone is "experienced reliable coworker" — fun, fast, practical, never lecturing.
- Emojis used sparingly to aid scanability, not for decoration.
- Positive and constructive. Frame in terms of what something *does*, not what others don't.
- Second-person, direct. No negative comparisons.

## Substrate constraints (architecture invariants)

These are positioning *invariants* — every command implements **with** them, not retrofits around them. Copied verbatim from the parent ticket.

1. **Plugin name on the marketplace card is `teamctl`** — not
   `teamctl-cc`, not `teamctl for Claude Code`, not
   `teamctl-onboarding`. The product name carries; the placement
   context (Claude Code marketplace) does the rest. Internal skill
   names stay descriptive (`/teamctl:init`, `/teamctl:adjust`).
2. **Reveal the YAML at end of onboarding.** Final beat of
   `/teamctl:init` says: *"I wrote `.team/team-compose.yaml` for
   you — open it, everything we just talked about is in there."*
   Keeps the team-as-code story honest on the conversational ramp.
3. **`.team/` output is byte-for-byte identical to a hand-authored
   team.** No plugin-specific markers, no
   `# generated-by: teamctl-cc-plugin` comments, no skill-only
   state. A user inspecting the file should not be able to tell
   it came from a plugin.
4. **`/teamctl:adjust` never locks users in.** Every action it takes is
   an action the user could have taken with
   `vim .team/team-compose.yaml`. No skill-only formats, no
   plugin-only state.

## Operational corollaries

- Plugin-only state is forbidden. State that needs to persist between sessions lives in the user's `.team/` tree, not anywhere this plugin owns.
- Every action either command takes is reproducible by hand-editing YAML. If you can't describe the change as a YAML diff, it doesn't belong in the plugin.
- Generated files carry no CLI-specific markers. No `# generated-by:` comments. No plugin signatures. The file should look like a careful human wrote it.

## Canonical flow — domain-over-function

`/teamctl:init` does not hand users a template menu **for domain discovery**. Stage 2 (the discovery conversation) walks the user through surfacing their own *domains* — things with their own state, history, and decisions — and synthesises a team from those. The cut is by domain ownership, not by job function. Free-form prose only at Stage 2; no template menu, no multiple-choice for the domain set itself.

**Intent routing is allowed to use option menus.** The intent on-ramp (the "Mode pick" question — `Co-design` / `Advanced co-design` / `Delegate a job` / `Just give me something`) is an `AskUserQuestion` menu by design. The invariant above protects the *discovery method*, not the on-ramp altitude — picking which path through `init` is a finite-set decision the user makes once, before discovery runs. See [INTERACTIVE.md §1](./INTERACTIVE.md) for when option menus are the right shape.

The legacy template-pick stage and the four named defaults (OSS maintainer, Editorial room, Indie studio, Solo triage) live on only as **inspirations**: shape-references the user can compare against after they've named their own domains. Never as pickable starts for the *team shape*.

The canonical companion is [`docs/src/content/docs/concepts/teams.md`](../../docs/src/content/docs/concepts/teams.md) — the methodology there is the source of truth. If anything in `/teamctl:init` drifts from that page, the page wins.
