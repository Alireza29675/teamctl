# Example: gastown-style

A teamctl team that expresses [Gas Town](https://github.com/gastownhall/gastown)'s seven-role shape in teamctl primitives.

This is a positioning artifact: teamctl is the meta-layer that lets you write a Gas Town in YAML. The example demonstrates the shape; operators who want to run it at Gas Town's scale will scale the agents themselves.

```
mayor (Claude Opus)              ← Telegram: mayor bot
  ├─ crew    (Claude Opus)       · #rig    : long-lived collaborator
  ├─ witness (Claude Opus)       · #rig    : rig supervisor, patrols polecats
  │   ├─ refinery (Claude Sonnet) · #rig    : merge queue
  │   └─ polecat  (Claude Sonnet) · #rig    : hooked-work worker
  └─ deacon  (Claude Opus)       · #town   : town-level patrol daemon
       └─ dog (Claude Sonnet)    · #town   : maintenance helper
```

## What you get

Seven agents in one project. Town-tier roles (mayor + deacon + dog) handle cross-rig coordination; rig-tier roles (crew + witness + refinery + polecat) handle the work itself. Channels keep traffic insulated: `#town` for the patrol-daemon side, `#rig` for the implementation side, `#all` for everyone.

Operator talks to `mayor` on Telegram. Mayor routes work down the org chart.

## Mapping cheat-sheet

The translation from Gas Town primitives to teamctl primitives:

| Gas Town | teamctl primitive | Notes |
|---|---|---|
| **Mayor** | Manager-tier agent with Telegram interface | Clean map. Receives operator DMs, routes work. |
| **Crew** | Manager-tier persistent named agent | Clean map. Long-lived; carries context across sessions. |
| **Witness** | Manager-tier agent with `reports_to` hierarchy + `can_dm` to polecats and refinery | Clean map. Patrol cadence lives in the role's Loop section. |
| **Polecats** | Worker-tier agents | **Reinterpretation.** Gas Town spawns polecats on demand and nukes them on `gt done`; teamctl agents are long-lived under tmux. The example ships a fixed pool that idles between tasks; the GUPP principle is preserved in the role prompt. |
| **Refinery** | Worker-tier agent with merge-queue logic in the role prompt + shell access to `gh pr merge` | **Approximation.** Bors-style bisect lives in the prompt; teamctl has no native merge-queue primitive. |
| **Deacon** | Manager-tier agent at the town tier | Patrol cadence is prompt-driven; teamctl has no native cron primitive. Cadence works because the loop section says it does. |
| **Dogs** | Worker-tier agents reporting to deacon | **Reinterpretation.** Gas Town's Dogs are imperative Go; teamctl version is agent-interpreted. The example ships one generic `dog`; specialized dogs (Doctor, Reaper, Compactor) are a follow-up split. |
| **Beads** | Mailbox messages + GitHub issues | **Different surface, similar function.** See "Beads as a different surface, not a missing primitive" below. |
| **Epics** | Parent-child issue links on GitHub | Approximated. No native hierarchy primitive in teamctl. |
| **Molecules** | The Loop section of each role prompt + chained DMs between agents | Reinterpreted. Multi-step workflows live in role-prompt prose + agent-to-agent handoffs. |
| **Formulas** | Skills (`plugins/claude-code/commands/*.md`) — partial coverage only | **Known v1 limit.** Skills are procedural prompts; formulas are templates that compile into chained workflow graphs at invocation. Composition isn't preserved in v1. |
| **GUPP** | "Read inbox first thing on every tick" — prompt-enforced | Behavioral; no structural enforcement layer in teamctl. The polecat role prompt names GUPP explicitly. |
| **`gt sling`** | `dm` MCP tool / `teamctl send` | Same effect, different ergonomics. |
| **`gt mayor attach`** | `tmux a -t gt-gastown-mayor` | Direct tmux. |
| **`gt feed`** | `teamctl ui` (TUI) | Approximated. |

## Beads as a different surface, not a missing primitive

Gas Town tracks work as a VCS artifact — beads are atomic units stored in Dolt (SQL), versioned in git, audit-trailed in the repo. teamctl tracks work as server-side state — mailbox messages in SQLite plus GitHub issues for the long-lived items.

Both are honest. The trade-offs:

- **Beads-in-git** give you an audit trail you can `git log`, offline capability, forkability. Cost: schema versioning, conflict resolution, and a custom workflow vocabulary.
- **Mailbox + GH issues** give you better UX out of the box, free search/filter/labels, and a stack most operators already know. Cost: GitHub lock-in for the persistent surface.

teamctl picked the GitHub path because the UX dividend is real and the audit trail (commit history + closed issues) is acceptable. Gas Town picked beads because git-as-database is core to its thesis. Different choices, both deliberate.

## Formulas: the known v1 limit

Gas Town's formulas are TOML templates that compile into chained workflow graphs at invocation time — `gt prime` shows a polecat its steps inline, lifted from the formula and customized to the bead. teamctl's closest adjacent is skills (`/teamctl:init`, the role-prompt-style guide, etc.) but skills are procedural prompts, not templates that compose.

For v1 of this example, formula-driven workflows are reinterpreted as prose in each role's Loop section. Composition (the formula-of-formulas pattern) is deliberately not preserved. Bringing native formula primitives into teamctl is a vision-track conversation, not a blocker for this example.

## Polecats and the ephemeral question

Gas Town spawns polecats on demand and nukes them on `gt done`. The model is per-task isolation: each work item gets a fresh worktree, a fresh session, a fresh sandbox.

teamctl agents are long-lived under tmux. The example ships a fixed pool of polecat workers that idle between tasks. GUPP — *"If there is work on your hook, YOU MUST RUN IT"* — is preserved in the polecat role prompt: the polecat reads its inbox, sees hooked work, starts immediately. The "fresh sandbox per task" property is approximated by the agent reloading its mental model from the work item itself, but a clean filesystem-level sandbox per task isn't part of v1.

Operators who want true on-demand spawn can split this example into multi-rig and use teamctl's existing supervisor primitives to manage the pool size dynamically — that's a scaling pattern, not a missing primitive.

## Scaling out: one rig to many

The example ships a single project (`projects/gastown.yaml`) holding all seven roles. Gas Town's actual architecture is two-tier: a Town hosting shared infrastructure (mayor + deacon + dogs), and one or more Rigs each with their own per-project agents (crew + witness + refinery + polecats).

Scaling this example to multi-rig:

1. Move mayor + deacon + dog into a separate project file (e.g. `projects/town.yaml`).
2. Move crew + witness + refinery + polecat into a per-rig project file (`projects/rig-alpha.yaml`, `projects/rig-beta.yaml`, ...).
3. Wire teamctl bridges between the town project and each rig so mayor can route to crew across projects.

That's a standard teamctl multi-project pattern; the [two-projects cookbook recipe](../../docs/src/content/docs/cookbook/two-projects.md) walks the bridge setup.

## Run

```bash
cp .team/.env.example .team/.env
# Edit .team/.env with your Telegram bot token + chat id for the mayor.

teamctl validate
teamctl up
```

Message the mayor bot on Telegram. Try:

> *"can you ask polecat to draft a README section about teamctl?"*

The mayor routes to polecat (via witness if you want the supervisor in the loop), polecat picks up the work under GUPP, hands off to refinery on completion, refinery merges. Observe the full path in `teamctl ui`.

## Customize

- Edit `roles/<name>.md` to tune any agent's voice, scope, or HITL gates.
- Edit `projects/gastown.yaml` to swap models, change ACLs, add a second polecat for more parallelism.
- Edit `team-compose.yaml` to wire additional interfaces or change broker / supervisor.

After any edit, `teamctl reload gastown` picks up the change.

## Not a competition

This is "here's how teamctl expresses this pattern," not "teamctl vs Gas Town." Gas Town is its own thing; the maintainers have chosen primitives that fit their thesis. teamctl is the meta-layer that lets you write a Gas Town in YAML — among many other shapes.

If you're running a real Gas Town, run Gas Town. This example is for operators who want to understand teamctl by reading something they already recognize.
