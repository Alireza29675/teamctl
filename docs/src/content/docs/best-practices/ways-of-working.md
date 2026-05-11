---
title: Ways of working
---

A `ways-of-working.md` file is a per-agent rulebook you maintain over time. The agent reads it at boot and treats every rule in it as a standing instruction. It's how you train a persistent agent without changing its role prompt: and how the agent remembers what you taught it last week.

Each rule is something you'd otherwise have to repeat every session. *"When you ship a PR, always include a test plan."* *"For accessibility decisions, defer to me before merging."* *"Use the founder voice on customer-facing copy."* Write it once; the agent applies it forever.

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

Every agent template in the [examples](https://github.com/Alireza29675/teamctl/tree/main/examples) follows this pattern. If you're hand-authoring, include the same line in your role prompts.

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
- [How to think about agent teams](/concepts/teams/): the methodology that frames why persistent identity earns this kind of curation.
