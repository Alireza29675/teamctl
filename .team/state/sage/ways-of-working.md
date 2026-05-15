# sage — ways of working

> Durable operator instructions. Re-read at the start of every
> tick. Append when the project owner gives you a standing rule
> ("from now on do X", "never do Y"). Quote their words. Add a
> short *why* / *how to apply* line. Remove entries that no
> longer apply.
>
> Otto (operations) may also edit this file when delivering a
> process change from the project owner. Treat otto's edits as
> ratified.

## Names, handles, emails, social links — always double-check, never guess

> "be very careful with people's names and emails and social links. don't use what you're unsure. always double check!" — owner, tg 1496 (relayed via hugo msg 1501)

**Why:** 0.8.0 release body draft mis-spelled Hamed's GitHub handle (wrote `HamedFathi`, actual is `hamifthi`). Owner caught it before publish. Reputation cost lands on the project; correction is friction for the contributor.

**How to apply:** before any issue body, PR body, release body, docs page, or chat message I'm authoring includes a contributor's name, handle, email, or social URL, verify the value from at least one of:
- commit co-author trailer (`git log --format=%B`)
- `gh pr view <n> --json author` or `gh issue view <n> --json author`
- `gh api users/<handle>` for handle round-trip
- owner-supplied profile URL or message

If none of those are available, ASK rather than guess. Extends existing rule against inferring a surname from a first name (`memory/feedback_credit_real_names_from_source.md`).

## Track Codex / Gemini parity gaps explicitly when shipping claude-only features

> "from now on we should keep track of what we are not implementing for Codex and Gemini so we can catch up with them later a bit faster." — owner, tg 1715 (2026-05-12)

**Why:** v1 of any per-runtime feature naturally lands on claude first (that's the runtime with the most code path and the most users today). Without explicit tracking, parity work for codex/gemini stays in vibes — engineers don't know the surface, and the gap silently widens with every claude-only ship.

**How to apply:** when any feature ships claude-only and has obvious codex/gemini analogues:

- The issue body's **Non-goals** AND a dedicated **Codex / Gemini parity gap** section name the deferred surface concretely (what would need to exist for each runtime, what's known vs. unknown).
- The parity items are filed as separate tickets when their shape becomes clear, or stay in the parity-gap section of the parent ticket as a tracking note when they don't.
- Future-sage should `grep` for "parity gap" / "claude-only" in open issues when surfacing release-readiness or roadmap rollups — surface the accumulated gap to owner periodically so it doesn't go invisible.

First applied: #212 (runtime usage indicator) — claude RL window v1, codex/gemini placeholder dash with explicit parity-gap section.


