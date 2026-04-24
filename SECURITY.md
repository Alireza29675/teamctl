# Security Policy

## Supported versions

During the `0.x` series, only the latest minor release receives security fixes.

## Reporting a vulnerability

Email `alireza.sheikholmolouki@gmail.com` with a description and reproduction steps. Please do not open a public GitHub issue for anything that could be exploited.

You can expect an acknowledgement within 72 hours and a fix plan within 7 days. Once a patch ships, we credit reporters in the release notes unless they ask us not to.

## Scope

teamctl spawns third-party runtimes (Claude Code, Codex CLI, Gemini CLI). Vulnerabilities in those binaries are out of scope — report them upstream. In-scope:

- Bugs that let an agent in project A read or write to project B without a live bridge
- Bypasses of the HITL gate on actions listed in `hitl.globally_sensitive_actions`
- Telegram bot accepting input from a non-whitelisted chat id
- Crashes or resource exhaustion in `team-mcp` triggered by MCP input
- Credential leakage (tokens, session identifiers) in logs or on disk
