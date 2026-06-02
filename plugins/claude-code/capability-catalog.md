# Capability Catalog

The palette the generator draws from when it emits a team's **per-agent capability stack** — sub-agents, skills, and the one earned hook. Sibling to [`role-prompt-style.md`](./role-prompt-style.md): that file is the shape guide for *role prompts*; this one is the shape guide for *capabilities*.

This is a **palette and a quality floor, never a fixed template.** The archetypes below are dogfood-proven — they're the shapes the teamctl team and the Ideate & Build starter already run. The generator **composes a bespoke stack from them on the fly**: it picks the archetypes that fit each agent's domain, **adapts** each to the team's own terms (never pastes verbatim), and falls to the **bespoke escape hatch** for the long tail where no archetype fits. Same posture as role prompts — read for shape, restate in the team's language.

The bounded surface (12 sub-agents · 3 skills · 1 hook) is deliberate: it gives the emit a quality floor and keeps generation from sprawling into freehand. The escape hatch covers everything the palette doesn't.

## How the generator uses this catalog

For each agent in the synthesised org (`init.md` Stage 4, `adjust.md` Verb 1 / Verb 8):

1. **Pick by domain.** Match the agent's domain and shape (builder vs. compass — see below) to the archetypes that earn their place. Lean: an archetype only goes in if the agent will actually reach for it.
2. **Materialize an adapted copy.** Write the file into the team's `.team/` — `subagents/<name>.md`, `skills/<name>/SKILL.md`, `hooks/<name>.sh` — adapting the body to the team's domain and vocabulary. Don't copy this doc's prose; author the real artifact.
3. **Declare the key, file-first.** *Write the file, then add its key to `projects/<id>.yaml`.* Paths resolve by construction — see the path-resolution note below. Then run `teamctl validate` (schema + team coherence) as the Stage-4 gate.

**Substrate-clean (constraint #3 holds):** an adapted archetype looks exactly like a careful human authored it. No `# generated-by:` markers, no plugin-only keys. A user reading `.team/subagents/qa-tester.md` cannot tell it came from a catalog.

### Path resolution — what validate does and doesn't catch

`teamctl validate` checks **schema and team coherence** (well-formed YAML, `reports_to` resolves, no `team`-named MCP clobber, role_prompt non-blank). It does **not** verify that a declared `subagents:` / `skills:` / `hooks:` path exists on disk — a dangling path validates green. So the emit can't lean on validate to catch a missing file.

Two things keep the emit honest instead:

- **File-first emit.** Always write the capability file *before* declaring its key, so every declared path resolves by construction. After emitting, the generator does a cheap self-check that each declared path exists.
- **Render is the backstop.** At `teamctl up` / `teamctl reload`, render reads each declared sub-agent file (`read_to_string`) and **hard-errors** on a missing one — so a mis-emitted sub-agent path fails loudly at bring-up, not silently at runtime. (Skills mount as symlinks and hooks render into settings; a missing one degrades rather than erroring, which is one more reason the file-first + self-check discipline matters.)

## Agent shapes — who gets a `/loop`

Two shapes drive which capabilities (and whether a `/loop`) an agent earns:

- **Builder-shaped** — an autonomous goal→ship drive. Handed a goal, it builds → tests → fixes → opens a PR. Builders earn the **build-side sub-agents** (`code-investigator`, `implementer`, `test-author`, `qa-tester`, `pr-narrator`, `code-roaster`), the **`ship-it`** + **`tdd`** skills, the **`fmt-lint`** hook (iff it writes code), and a **`/loop`** as their heartbeat (see [`role-prompt-style.md`](./role-prompt-style.md) §5).
- **Compass-shaped** — conversational / event-driven: ideation, research, triage, coordination. No autonomous build drive, **no `/loop`**. Earns the **research / ideation sub-agents** (`product-researcher`, `feasibility-analyst`, `deep-research`, `memory-writer`, `ideator`) and the **`shape-idea`** skill; a manager who forwards work earns **`pr-summarizer`**.

Most teams are one or two builders plus a compass-shaped manager. Keep it lean (RULES: capabilities-over-seats).

---

## Sub-agent archetypes

Declared per agent as a list of compose-root-relative paths:

```yaml
subagents:
  - subagents/code-investigator.md
  - subagents/implementer.md
```

Each file is a standard Claude Code sub-agent: frontmatter `name` / `description` / `tools` (optional `model`), body = the sub-agent's system prompt. The generator authors an adapted body from the **shape** column below — a few sentences of real system-prompt guidance in the team's terms, with the same read-only-vs-writes posture.

### Build-side (a builder-shaped agent's stable)

| Archetype | When the agent reaches for it | tools | Body shape (adapt to the domain) |
|---|---|---|---|
| `code-investigator` | First move on any change — map the terrain before touching code. | `Read, Grep, Glob` | Maps where things live, what calls what, what a change would touch. Returns a short orientation map. **Read-only; never edits.** |
| `implementer` | A precise, well-scoped change is decided and you want the diff typed. | `Read, Grep, Glob, Edit, Write, Bash` | Takes a concrete spec + judgment from the caller and writes the diff. Returns the change. The caller owns the design; this types it. |
| `test-author` | Coverage for a change — happy path, edges, failure modes — in the same PR. | `Read, Grep, Glob, Edit, Write, Bash` | Writes/extends tests for a change or spec, following the repo's existing test patterns. |
| `qa-tester` | Before calling a change ready — exercise it like a skeptical user. | `Read, Grep, Glob, Bash` | Black-box, adversarial: runs the suite + exercises the change to find the failure the author missed. **Reports; never fixes.** |
| `pr-narrator` | A finished diff needs a clear, human PR body. | `Read, Grep, Glob, Bash` | Turns the diff into what-changed / why / how-to-verify. Returns ready-to-paste markdown. No AI attribution. |
| `code-roaster` | A hard self-review before a human sees the branch. | `Read, Grep, Glob, Bash` | Adversarial pass over the diff — bugs, edges, sloppy names, scope creep — ranked by severity. **Read-only.** |

### Compass-side (research / ideation / coordination)

| Archetype | When the agent reaches for it | tools | Body shape (adapt to the domain) |
|---|---|---|---|
| `product-researcher` | Ground an idea in prior art, competitors, what users expect. | `Read, Grep, Glob, WebSearch, WebFetch` | Researches what exists; returns conclusions with sources. **Read-only.** |
| `feasibility-analyst` | Pressure-test whether an idea can actually be built — effort + risk. | `Read, Grep, Glob, WebSearch, WebFetch` | Reads the real code, returns a grounded buildability estimate. **Read-only.** |
| `deep-research` | An idea needs the full landscape, not a quick scan. | `WebSearch, WebFetch, Read, Grep, Glob` | Multi-source synthesis of a topic/market/domain; returns a cited brief. **Read-only.** |
| `memory-writer` | Capture an idea (or fragment) to committed memory while the conversation continues. | `Read, Write, Edit, Glob` | Writes idea notes to `.team/ideas/<slug>.md`. **Writes only what it's fed; never invents.** |
| `ideator` | Quiet stretches — surface concrete proposals worth raising. | `Read, Grep, Glob, WebSearch, WebFetch` | Generates ranked product/feature pitches within the settled scope. **Pitches, never decisions.** |
| `pr-summarizer` | A forwarding manager turning a ready PR into an approve-at-a-glance summary. | `Bash, Read, Grep, Glob` | Plain-language PR summary a non-engineer can approve. **Read-only.** |

### Bespoke escape hatch

When an agent's domain has a fire-and-forget specialty no archetype fits — a `schema-migrator`, a `dataset-profiler`, a `lint-rule-author` — **generate it on the fly**, exactly like a role prompt: name it for the work, give it the narrowest tool set that does the job, write the system prompt in the team's terms, default to **read-only** unless the work genuinely needs `Edit`/`Write`. The escape hatch is for the genuine long tail, not a reason to skip the proven archetypes.

---

## Skill archetypes

Declared per agent as a list of compose-root-relative paths to the skill **directory** (the folder holding `SKILL.md`):

```yaml
skills:
  - skills/ship-it
  - skills/tdd
```

Each is materialized as `skills/<name>/SKILL.md` (frontmatter `name` / `description`, then the body). The generator adapts the flow to the team's stack and vocabulary.

| Skill | Goes to | What it does |
|---|---|---|
| `ship-it` | builders | Drives a handed goal to a shipped PR: **plan → `/loop` build-verify → open PR → `request_approval`**. Its *ship it / make changes / pick a path* decisions use **`AskUserQuestion`** in an interactive session (prose fallback when non-interactive). This is how the emitted team inherits the same interactive-decision UX the plugin uses. |
| `tdd` | builders | Failing-test-first: write the test that captures the goal, watch it fail, make it pass, refactor green. Pairs with `test-author`. |
| `shape-idea` | compass-shaped | Capture → research → settle intent → hand off. Turns a raw idea into a shaped, build-ready brief for a builder; leans on `product-researcher` / `feasibility-analyst` / `memory-writer`. |

For the tightest cut, a build team ships with **`ship-it`** alone; add `tdd` when the team values test-first, `shape-idea` when a compass-shaped agent shapes work before it reaches builders.

---

## Hook archetypes — earned only

Hooks are the one capability that is **off by default.** A hook fires on every matching tool call, so it only earns its slot when it pays for itself. **`fmt-lint` is the only hook in v1** — the generator does not sprinkle hooks beyond it.

Declared per agent:

```yaml
hooks:
  - event: PreToolUse
    matcher: "Edit|Write"
    command: hooks/fmt-lint.sh
```

| Hook | Earns its slot when | Shape |
|---|---|---|
| `fmt-lint` | An agent **writes code** — format + lint every edit before it lands. | `PreToolUse` on `Edit\|Write` → `hooks/fmt-lint.sh`. Formats the file in place, lints the scriptable ones; exits `2` (blocks the edit, feeds the reason back) only on a real lint failure. **Missing dev tools degrade to a warning, never a hard block** — the gate sharpens the team, it never wedges a fresh checkout. |

The script is adapted to the team's stack — swap the formatter/linter and the file-extension match for the team's languages. Canonical skeleton (adapt the `case` arms and the `prettier`/`eslint` calls; keep the degrade-to-warning posture and the `chmod +x`):

```bash
#!/usr/bin/env bash
# PreToolUse fmt+lint gate. Claude Code passes the tool input as JSON on
# stdin; format the file being written and lint it. A real lint failure
# exits 2 (blocks the edit, reason fed back). Missing tools warn, never block.
set -euo pipefail
input="$(cat)"
if command -v jq >/dev/null 2>&1; then
  file="$(printf '%s' "$input" | jq -r '.tool_input.file_path // empty')"
else
  file="$(printf '%s' "$input" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
fi
[ -n "${file:-}" ] || exit 0
[ -f "$file" ] || exit 0
case "$file" in
  *.<ext1> | *.<ext2>)   # ← the team's source extensions
    command -v <formatter> >/dev/null 2>&1 && <formatter> --write "$file" >/dev/null 2>&1 \
      || echo "fmt-lint: formatter unavailable on $file (skipping)." >&2
    if <linter-is-installed-probe>; then
      <linter> "$file" >&2 || { echo "fmt-lint: lint problems in $file — fix before this edit lands." >&2; exit 2; }
    else
      echo "fmt-lint: linter not installed; skipping lint gate." >&2
    fi
    ;;
esac
exit 0
```

---

## The mapping, at a glance

```yaml
managers:
  <builder>:
    role_prompt: [roles/_base.md, roles/<builder>.md]
    subagents: [subagents/code-investigator.md, subagents/implementer.md, subagents/test-author.md,
                subagents/qa-tester.md, subagents/pr-narrator.md, subagents/code-roaster.md]
    skills:    [skills/ship-it, skills/tdd]      # path = the skill DIR, not SKILL.md
    hooks:
      - event: PreToolUse
        matcher: "Edit|Write"
        command: hooks/fmt-lint.sh               # chmod +x on emit
```

`subagents` → `.md` files · `skills` → skill **directories** · `hooks` → `{event, matcher, command}` objects. (The PathBuf/HookSpec shapes ship in #383; the generator only emits them.)

## What the catalog deliberately excludes

The generated team's output stays clean (RULES: *no cron / no extra-MCP in the emitted team*):

- **No cron.** `/loop` is the cron-free heartbeat — a builder self-paces its goal→ship drive in `/loop` dynamic mode (`role-prompt-style.md` §5). The generator never emits a cron block.
- **No extra MCP servers.** The built-in `team` mailbox MCP is all an agent needs. The generator never emits an `mcps:` server. (A user can add one by hand later; the generator doesn't reach for it.)

These are guards on the **generated team**, not on the plugin — the plugin itself uses whatever it needs.
