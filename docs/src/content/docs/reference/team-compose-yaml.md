---
title: "Reference: agent configuration"
---

The global `.team/team-compose.yaml` selects the broker, web workspace, named interfaces and project files. Each project defines agents, channels and its work directory. Schema3.0.0 uses a single `agents` map; hierarchy and permissions are separate.

```yaml
version: "3.0.0"
broker:
  path: state/mailbox.db
web:
  host: 127.0.0.1
  port: 8787
  cache_retention_days: 30
  max_upload_bytes: 26214400
projects:
  - file: projects/personal.yaml
interfaces:
  - id: private-phone
    provider: telegram
    name: Private phone
    description: Personal notes and requests
    config:
      agent: personal:companion
      bot_token_env: PRIVATE_BOT_TOKEN
      authorized_chat_ids_env: PRIVATE_CHAT_IDS
```

Every interface has a stable instance ID, provider, name and short description. Multiple instances of one provider are independent. Bot configuration refers to secret environment variable names; their values belong in the ignored `.team/.env` or the parent process environment. Web is automatic and available to every defined agent. Incoming messages snapshot all four provenance fields. Agent `list_interfaces` returns its grants; `reply_to_user` inherits the original conversation or selects `interface_id`. An arbitrary unknown interface cannot receive messages.

```yaml
version: 1
project:
  id: personal
  name: Personal workspace
  cwd: ../workspace
agents:
  companion:
    runtime: codex # claude-code | codex | opencode
    display_name: Companion
    role_prompt: roles/companion.md # or a list, concatenated in order
    autonomy: low_risk_only
    execution:
      mode: native # tmux for an interactive long-lived CLI
      turn_timeout_seconds: 300
      wake_interval_seconds: 900 # omit for incoming-message turns only after first wake
      sandbox: {type: host}
    can_dm: [researcher]
    can_broadcast: [notes]
    mcps:
      local-notes:
        command: node
        args: [./notes-mcp.mjs]
        env_vars: [NOTES_API_KEY]
      remote-notes:
        url: https://mcp.example.com/mcp
        bearer_token_env_var: NOTES_TOKEN
  researcher:
    runtime: opencode
    reports_to: companion
    can_dm: [companion]
    can_broadcast: [notes]
channels:
  - name: notes
    members: "*" # or [companion, researcher]
```

Agent IDs are local to their project; canonical routing IDs look like `personal:companion`. `reports_to` can target any other agent in the same project and must not form a cycle. It describes relationships; it does not grant messaging or tool permissions. `can_dm` names allowed peers; `can_broadcast` names channels this agent may publish to. Receiving channel messages requires membership.

Each agent has a runtime, optional model, optional role prompt, optional tools/MCPs and execution configuration. Omit model to use the runtime's own default; a model appearing in local metadata does not prove account access. Persistent provider sessions and durable local notes are different layers: keep task lists/checkpoints on disk so work survives compaction or a new session.

MCP stdio connections use command/args/env and optional `env_vars` references. Remote MCP connections use `url`, optional `bearer_token_env_var` and `env_http_headers` mapping header names to environment variable names. Never embed secret values in YAML. Provider-specific rendering preserves references; missing declared environment variables fail before launch. HTTP MCP URLs require HTTPS except local loopback endpoints. Tool servers own their own authorization; teamctl does not invent OAuth grants.

For Docker images, explicit mounts, credentials, timeouts, scheduling and runtime differences, see [native agents](../../guides/native-agents/). The [personal agent](https://github.com/Alireza29675/teamctl/tree/main/examples/personal-agent) is the complete starter recipe.
