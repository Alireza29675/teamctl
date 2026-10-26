---
title: Your first agent
---

Start with `teamctl init my-workspace --yes`. The personal recipe creates one agent, a role prompt and a durable workspace. Authenticate its chosen runtime CLI, then run `teamctl up` and `teamctl serve`.

Companion organizes information in ABOUT, INBOX, TASKS, MEMORY, JOURNAL and topical notes. Messages and scheduled wakes each trigger one bounded turn. Durable checkpoints let it resume after compaction.

Add agents to the project's `agents` map when you need distinct capabilities. Grant communication with `can_dm`, channel membership and `can_broadcast`. Use `reports_to` if a hierarchy helps; every node remains an agent.

See the [complete personal recipe](https://github.com/Alireza29675/teamctl/tree/main/examples/personal-agent), [schema](../../reference/team-compose-yaml/) and [web workspace](../web-workspace/).
