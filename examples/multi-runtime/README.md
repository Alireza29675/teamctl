# Example: multi-runtime

One manager talking to two workers — **each running a different CLI**:

| Role | Runtime | Good for |
|---|---|---|
| `manager` | Claude Code · Opus | planning, orchestrating, long system prompts |
| `backend` | Codex CLI · GPT-5 | deep backend reasoning, complex patches |
| `researcher` | Gemini CLI · 3.0 Pro | 1M-token context for research |

All three agents join the same SQLite mailbox and talk through the same MCP tools. The manager doesn't know — or care — that its workers run on different stacks.

```bash
teamctl validate
teamctl up
teamctl send mixed:manager "research X and hand to backend to implement"
```

## Prerequisites

Install each CLI you want to use. `teamctl up` fails fast with a clear error if a runtime binary is missing.

- Claude Code — `npm i -g @anthropic-ai/claude-code`
- Codex CLI — see OpenAI's install docs
- Gemini CLI — see Google's install docs
