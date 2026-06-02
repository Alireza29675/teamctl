# INTERACTIVE

Shared interactive-UI substrate every command in this plugin reads off. Both `/teamctl:init` and `/teamctl:adjust` honour every rule below. Sits alongside [RULES.md](./RULES.md) — RULES carries the architecture invariants; this file carries the UI invariants.

The motivation: the two skills drive every operator's first hour with teamctl. Concise on the question step, comprehensive on the proposal step, explicit on the accept/reject moment. A skill that nails this feels like a teammate; one that doesn't feels like a wizard.

## 1. Use interactive primitives for finite-set picks

When the answer space is finite (yes/no, three install paths, a manager from a known set, Apply/Modify/Reject), reach for [`AskUserQuestion`](https://docs.claude.com/en/docs/claude-code/sdk#ask-user-question) — never free-form *"tell me what you want"* prose. Free-form is for the discovery beats (what domains do you own? what would silently rot?) — anywhere the answer truly is open.

Use free-form when:

- The user is naming things in their own words (Stage 2 discovery).
- The user is describing voice / style / preference in a paragraph.
- There's a genuinely open answer space.

Use `AskUserQuestion` when:

- The options are knowable in advance (≤4 choices).
- A yes/no or accept/reject confirmation is the right shape.
- The user is picking *which* of something, not *what* something is.

## 2. Question shape

Tight surface; the user is here to decide, not to read.

- **Header** ≤ 12 chars (a chip / tag — e.g. `Install path`, `Manager`, `Apply?`).
- **Question** one sentence, ends with `?`.
- **Option label** ≤ 5 words, distinct, mutually exclusive.
- **Option description** one short sentence — explain the consequence, not the mechanism.
- **2–4 options.** No more. If the answer space is wider, narrow first or fall back to free-form.

Don't preface with apologies or padding. The chip + question is the whole beat.

## 3. Comprehensive reasoning lives *before* the gate, not inside it

When you're proposing structure — a team shape, a YAML diff, a role-prompt sketch — the reasoning is comprehensive prose **before** you open the gate. The gate itself stays tight.

Cycle:

1. **Propose** (comprehensive). Plain English, 1–2 paragraphs if the change is substantial. Cite the relevant docs/concepts page (see §6). Name the trade-offs you considered. Show the YAML diff or the team tree as the receipt.
2. **Gate** (tight). The Apply/Modify/Reject question. No re-explanation, no recap; the proposal already did that.

The proposal carries the weight; the gate carries the decision.

## 4. The Apply/Modify/Reject gate

Every flow that mutates state — writes a file, edits YAML, runs `teamctl up` — ends with this gate. Universal across both skills.

Shape:

```text
question: "Apply this change?"
header: "Apply?"
options:
  - label: "Apply"
    description: "Write the change and run `teamctl validate`. On green, advance."
  - label: "Modify"
    description: "Tell me what to adjust and I'll re-propose."
  - label: "Reject"
    description: "Discard. Nothing is written."
```

Branches:

- **Apply** → execute the change. On `teamctl validate` exit 0, advance; on non-zero, surface the error verbatim and offer rollback (don't re-prompt the gate).
- **Modify** → loop back to the proposal step. Ask one cutting follow-up question (free-form is fine here — the user is describing the change) and re-propose. The gate fires again on the refined proposal.
- **Reject** → write nothing. Acknowledge in one sentence and exit the flow.

A flow may show multiple gates if it mutates state in multiple beats (e.g. `/teamctl:init` Stage 4 scaffold-and-validate, Stage 5 bring-up). Each gate is independent.

**The three branches are semantic, not the literal labels.** A flow may use labels that read naturally for its context as long as the three branches map cleanly: *advance-and-execute* / *loop-back-to-refine* / *discard*. The init Stage-3 setup-approval gate uses **Looks good / Refine it / Start over**; the universal default is **Apply / Modify / Reject**. When a gate relabels, its headless prose (§5) mirrors *its own* labels, not the default ones.

## 5. Headless-pane fallback

A `teamctl` agent invoked from inside a supervised tmux pane has no human at its keyboard. Issue [#189](https://github.com/Alireza29675/teamctl/issues/189) lands a `PreToolUse` deny rule for `AskUserQuestion` / `EnterPlanMode` / `ExitPlanMode` in wrapper-managed `.claude/settings.json` — synchronous interactive prompts strand the pane and freeze the team's `request_approval` loop.

If `/teamctl:adjust` or `/teamctl:init` is invoked from inside such a pane (a teamctl agent reaching for the skill to evolve the team programmatically), every `AskUserQuestion` will be denied. Detect this up-front and fall back to plain-text Q&A for the whole invocation.

**Detection — at the top of every invocation, run:**

```bash
if [ -n "$TEAMCTL_ROOT" ] && [ -n "$AGENT_ID" ]; then
    echo "headless"
else
    echo "interactive"
fi
```

Both env vars are written into the agent's env-file by `teamctl render` (`AGENT_ID` = `<project>:<agent>`) and read into the spawned claude session by `agent-wrapper.sh`; they're absent in a normal user `claude` session. If the probe prints `headless`, set the invocation mode to **plain-text** and don't call `AskUserQuestion` even once.

**Fallback shape — plain-text Q&A.** Render every question as a numbered prose prompt the model can answer inline:

```text
Install path — pick one:
  1) `brew install teamctl` (macOS with brew on PATH).
  2) `curl -fsSL https://teamctl.run/install | sh` (Linux / WSL / macOS without brew).
  3) `cargo install teamctl team-mcp team-bot` (sandboxed / locked-down envs).

Reply with the number, or describe a different path.
```

The Apply/Modify/Reject gate becomes:

```text
Apply this change? Reply `apply`, `modify` (and say what to adjust), or `reject`.
```

Same questions, same options, same branches — just prose instead of the picker. A gate that relabels its branches (§4) prose-falls-back to *its own* labels — e.g. the init setup gate becomes *"Happy with this setup? Reply `looks good`, `refine` (and say what to adjust), or `start over`."*

**Belt-and-braces.** If detection misses (env var unset by some future variant) and `AskUserQuestion` is denied at runtime, treat the deny as the headless signal: switch the rest of the invocation to plain-text and don't retry the picker.

## 6. Docs/concepts as ground truth

When proposing structure — a team shape, a role boundary, a channel scope, a HITL gate — read the relevant concept page first and cite it in the proposal prose. The concept pages live at `docs/src/content/docs/concepts/*.md`; the canonical ones for the two skills are:

| When you're proposing… | Read… |
|---|---|
| A team shape or agent boundary | [`concepts/teams.md`](../../docs/src/content/docs/concepts/teams.md) — the domain-over-function methodology, the two-gate framing, the ship-alone test. |
| A channel members list or broadcast scope | [`concepts/channels.md`](../../docs/src/content/docs/concepts/channels.md) |
| A Telegram interface block | [`concepts/interfaces.md`](../../docs/src/content/docs/concepts/interfaces.md) |
| A `globally_sensitive_actions` change or `autonomy` field | [`concepts/hitl.md`](../../docs/src/content/docs/concepts/hitl.md) |
| A `reports_to` hierarchy | [`concepts/teams.md`](../../docs/src/content/docs/concepts/teams.md) |
| A new project entry or bridge | [`concepts/projects.md`](../../docs/src/content/docs/concepts/projects.md), [`concepts/bridges.md`](../../docs/src/content/docs/concepts/bridges.md) |
| A `runtime` choice | [`concepts/runtimes.md`](../../docs/src/content/docs/concepts/runtimes.md) |

Citation pattern in the propose beat — one short link or a single-sentence paraphrase, anchored to the canonical line. Example:

> Adding `docs` as a worker reporting to `maintainer`. Per [the teams methodology](../../docs/src/content/docs/concepts/teams.md), persistent agents earn their slot by owning a domain end-to-end — `docs` clears the ship-alone test (it ships the docs surface alone, consuming everything else as substrate).

Don't quote a wall; quote the line that's load-bearing for *this* proposal. If no concept page covers the change (a one-off cosmetic edit, a typo fix in a role prompt), no citation is needed.

## 7. Team-structure proposals — reasoning depth

When proposing a team shape (init Stage 3, or adjust's `Hire a new agent` branch), the proposal must reason about three things, in this order:

1. **Where the cut lives.** Why is this agent its own role and not a sub-agent? Reference the two-gate criteria — Gate (a) entry conditions (ownership / time management / persistent memory) and Gate (b) at least one situational trigger (domain separation, focus separation, multiple opinions, synergy). Apply the **ship-alone test**: can this agent ship its artifact alone? If no, the cut is function-shaped — call that out and propose a domain-shaped alternative.
2. **The capability stack.** Persistent agents own domains; their **capabilities** handle the fire-and-forget specialised work inside them. For each proposed agent, name its stack from [`capability-catalog.md`](./capability-catalog.md): **(i) sub-agents** (1–3 shapes — research passes, large refactors, parallel reads; e.g. *"`maintainer` would spawn `repo-cartographer` for new tickets and `regression-scanner` on each PR"*), **(ii) skills** (repeatable routines like `ship-it` / `tdd` / `shape-idea`), **(iii) whether it runs a `/loop`** (builder-shaped, autonomous goal→ship drive) or stays event-driven, and **(iv) the one earned hook** (`fmt-lint`, iff it writes code). The lean default is **capabilities-over-seats**: more work for an existing agent is a sub-agent/skill on it, not a new seat. The generated team carries **no cron and no extra MCP** — `/loop` is the heartbeat.
3. **What channels this agent participates in.** Which channels they're on (`can_dm` / `can_broadcast`), whether they're Telegram-bound (manager-only). Tie this back to the operator's discovered domains.

The propose beat is comprehensive on all three. The gate stays tight.

## 8. Voice control — concise on questions, comprehensive on proposals

Inherits everything in [`role-prompt-style.md`](./role-prompt-style.md) and the RULES voice rails. The added rule for interactive flows:

- **Question beats** (`AskUserQuestion` calls, the gate, free-form prompts): 1 sentence. No preamble, no recap, no apology.
- **Proposal beats** (the propose/synthesise step before a gate): 1–2 paragraphs. Comprehensive — name the reasoning, the alternatives considered, the docs citation, the receipt (YAML diff or team tree). The proposal is the load-bearing surface; the gate is the click.

The asymmetry is deliberate. The user reads the proposal carefully and clicks the gate quickly.
