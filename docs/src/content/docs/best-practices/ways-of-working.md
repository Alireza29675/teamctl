---
title: Ways of working
---

A `ways-of-working.md` file records rules you want an agent to keep using. Its role prompt should tell it to read the file on startup and apply those instructions. TeamCTL does not automatically enforce the file's contents.

Write rules you would otherwise repeat: include a test plan with a PR, ask before merging an accessibility change, or use a specified voice in public copy. Keep the rules current and check that the agent follows them.

## Where it lives

For each role on your team, the file lives at:

```
.team/state/<role>/ways-of-working.md
```

Where `<role>` is the agent's id from `team-compose.yaml` (e.g. `auth`, `editor`, `buddy`). The `.team/state/` tree is gitignored by default: these files are yours, local, and not shared unless you choose to commit them.

## Shape of the file

Free-form markdown. No required schema. Keep entries short, specific, and dated when context matters.

```markdown
# Ways of working: auth

## Always-on
- Before any change to the user model, surface to vision first.
- Pair every new public endpoint with a rate-limit decision.
- HITL on every action that affects session storage, not just deploys.

## Currently working on
- (2026-05-09) Migration to UUIDv5 session ids: keep back-compat for 30 days.

## Decisions worth remembering
- (2026-04-22) Owner ratified: no email-on-signup; magic link only.
- (2026-04-15) Decided against social OAuth for v1.
```

## Why it works

Persistent agents have memory, but memory degrades. A `ways-of-working.md` is the **operator-curated** layer above whatever the agent has accrued on its own: the rules *you* want to remain stable across sessions. The agent re-reads it on every restart; you edit it whenever you've taught the agent something worth remembering.

It's also the cleanest way to encode durable preferences without rewriting role prompts. Role prompts define identity and domain; ways-of-working captures the day-to-day rules a real teammate would just *know* after a few weeks of working together.

## How agents use it

A good role prompt has a line like:

> Read `.team/state/<your-role>/ways-of-working.md` at boot and apply every rule there as a standing instruction.

The [examples](https://github.com/Alireza29675/teamctl/tree/main/examples) show ways to keep per-agent context. If you're hand-authoring, include the reading instruction in the role prompt and check the resulting behavior.

## When to update it

- After every correction you give the agent. *"Don't ship without the test plan"* becomes a rule.
- After a decision worth remembering. The agent shouldn't need to ask you the same question twice.
- When the agent does something well that you'd like them to keep doing. Reinforcement matters as much as correction.
- When context shifts. *"We're freezing non-critical merges until Tuesday"* is a temporary rule; date it and remove it after.

## What NOT to put in it

- **Role identity**: that belongs in the role prompt. Ways-of-working is rules, not identity.
- **Project-wide invariants**: if it applies to *every* agent, put it in the role prompt of each, or in a shared spine. Ways-of-working is per-agent.
- **Secrets, tokens, credentials**: `.team/.env` exists for that.

## Related

- [The `.team/` folder](/concepts/team-folder/): where ways-of-working files live in the broader directory layout.
- [Designing an agent team](/concepts/teams/): the methodology that frames why persistent identity earns this kind of curation.
