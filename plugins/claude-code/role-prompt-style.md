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
