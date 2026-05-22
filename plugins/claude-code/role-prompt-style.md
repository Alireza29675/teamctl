# Role-prompt style guide

Every role prompt this plugin generates follows the same 8-section spine. The guide tunes over time — the project owner and pm refine it as the team sees real generated prompts; the commands always read the latest version of this file.

Voice rails on the prompts themselves: positive, constructive, second-person, no negative comparisons. Built **around roles, not tasks**.

## The 8 sections (in order)

### 1. Identity

Who you are, the team you're in, who you report to. One short paragraph; names the role, the team, the manager (if any), and the peers.

### 2. Mission

1-2 sentences. What success looks like for this role.

### 3. Voice

Default coworker baseline: slack-style, short, concise, clear, emoji-friendly, proactive in sharing and checking with stakeholders, "experienced reliable coworker." If the user picked custom voice for this manager during Stage 6 of `/teamctl:init`, the override lands here.

### 4. Best practices

5-8 bullets. Role-specific habits drawn from generally-accepted craft for that role (a maintainer's habits differ from an editor's, which differ from a designer's).

### 5. Loop

How the agent operates when nothing's pending. The idle behaviour — what to read, when to surface, when to wait.

### 6. Memory

The state file (one) plus painpoints (separate files per painpoint, written to `.team/state/<role>/painpoints/YYYY-MM-DD-<title>.md` so pm/eng_lead can pick them up as discrete signals).

### 7. Boundaries + HITL gates

In-scope, out-of-scope, actions that pause for operator approval (publish, release, deploy, payment, external messages).

### 8. Hard rules

Never-do list — security, scope, footguns. The non-negotiables.

## Tuning notes

- Keep the spine even when a section is brief; structure matters more than length.
- Custom voice overrides only touch section 3; everything else follows the role-driven defaults.
- Painpoint memory (section 6) is one-file-per-painpoint deliberately, so pm/eng_lead can route them as discrete signals rather than as one rolling log.

## Cascade — which sections live where

`/teamctl:init` writes role prompts as a cascade — universal → (workers only) role-tier → individual — consumed by `team-core`'s render layer as `RolePrompt::Multiple`. The cascade is additive: each tier contributes its own sections; the per-agent file references rather than duplicates the shared ones.

**Naming convention** (lexical — `ls roles/` shows the cascade base at the top):

- `_base.md` — universal, written once per team; every agent gets it.
- `_worker.md` — worker-tier shared, written once per team if any workers exist.
- `<name>.md` — individual agent file (`nico.md`, `ada.md`, …); no underscore prefix.

Underscore prefix = shared / cascading base. No prefix = individual agent. The convention is enforced by `init`'s Stage 4 emission and documented here so hand-edits stay consistent.

**Tier-shape asymmetry — managers vs. workers.** Managers get a 2-tier cascade (`[_base, <name>]`); workers get 3 (`[_base, _worker, <name>]`). Managers don't get a `_manager.md` tier file because manager craft is too distinct between roles (a `pm` and an `editor` and a `pi` share almost nothing useful at the tier altitude); their kind-generic content sits in `_base.md`, everything else stays per-manager in `<name>.md`. Workers share `_worker.md` because the worker shape (scoped execution, surface blockers, ship-then-archive) generalises cleanly. Matches the already-shipped dogfood convention (#295).

**Section distribution across tiers** (Stage 4 of `/teamctl:init` emits per these rules):

| Section | `_base.md` (universal) | `_worker.md` (workers only) | `<name>.md` (individual) |
|---|---|---|---|
| §1 Identity | — | — | **here** |
| §2 Mission | — | — | **here** |
| §3 Voice | — | — | **here** (custom-voice override land zone) |
| §4 Best practices | — | **here** for workers (kind-generic craft) | managers: full list; workers: agent-specific bullets only |
| §5 Loop | — | **here** for workers (kind-generic idle pattern) | managers: full loop; workers: agent-specific overrides |
| §6 Memory | layout / state-file / painpoint convention | — | agent-specific specifics only |
| §7 Boundaries + HITL gates | **here** (`globally_sensitive_actions`) | — | managers: full block; workers: agent-specific scope only |
| §8 Hard rules | universal-to-every-agent | worker-tier hard rules | agent-specific footguns only |

The cascade is **additive**, not duplicative. Sections fully covered by `_base.md` or `_worker.md` become *references* in `<name>.md` ("see `_base.md` §Memory; specifics for this agent below…"), not copies. This keeps individual files short and makes shared edits flow through one file.

Stage 6 voice customization touches only `<name>.md` (§3). The cascade base files are byte-stable across a voice change.
