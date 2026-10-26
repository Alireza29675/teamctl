---
title: Agents and teams
---

A team is a set of persistent agents working in the background and exchanging messages. Every node is an agent with a runtime, role instructions, work directory and tools. Start with one useful agent and add peers when distinct capabilities help.

Hierarchy is optional. `reports_to` describes a relationship to another agent in the same project. Direct-message ACLs, channel membership and broadcast grants independently describe communication. A hierarchy does not imply tool access or a separate privileged agent class.

Human interfaces are grants alongside those communication permissions. Native workers take bounded turns and release the model process when idle. Interactive tmux remains available for provider terminals.

The repository includes one [personal recipe](https://github.com/Alireza29675/teamctl/tree/main/examples/personal-agent), with a task list and recovery journal. See [configuration](../../reference/team-compose-yaml/) for tools, channels and execution.
