---
title: Designing an agent team
description: Define peers, project teams and organizations around your work.
---

TeamCTL is an open-source platform for autonomous teams of agents. You choose the roles, relationships and communication. The platform runs their sessions and carries messages between them.

A team can be two researchers questioning each other, a lead coordinating builders and reviewers, or an organization with several project rosters. These are configurations you can create and share.

## Start with the interaction

Describe what should happen between the agents before deciding how many to run.

- **Research partners:** one investigates a question; another checks sources and challenges the conclusion. They communicate as peers.
- **A project team:** a lead defines work and follows progress. Builders and reviewers exchange questions and results.
- **An organization:** shared role files describe responsibilities, while separate project rosters define working directories and communication scopes.

Use a small team first. Add an agent when a separate perspective, responsibility or conversation makes the work clearer.

## Define roles and relationships

Role files explain what each agent owns, how it responds to messages and when it should ask you for direction. Specify the tools and access it needs alongside those instructions.

`reports_to` records a reporting relationship. It is optional: peer agents can communicate without a manager. A reporting line alone does not create a delegation workflow; the role instructions describe how the agents use it.

Channels give a group a shared conversation. Direct-message and broadcast permissions limit where each agent may send messages. Project isolation separates unrelated work; [bridges](/concepts/bridges/) permit a bounded conversation between managers in different projects.

## Choose what benefits from persistence

Each TeamCTL agent is a named runtime session with its own conversation history. The supported runtimes are Claude Code, Codex, OpenCode and Gemini CLI. Session-resume support varies by runtime; see [Runtimes](/concepts/runtimes/).

A persistent session is useful when context builds over time: an ongoing research question, a project with changing requirements, or a domain with decisions worth keeping. Role instructions can tell the agent how to maintain notes and read them on restart.

Sub-agents are useful for bounded work within a runtime session. A TeamCTL agent can use its runtime's sub-agents while staying connected to the rest of the team. Decide which work needs a continuing identity and which fits a temporary helper.

## One useful pattern: domain ownership

For a long-running organization, give each agent a clear area of responsibility. Authentication, billing or the documentation site can each have an owner who builds, reviews and maintains context for that area.

Three questions help decide whether that boundary is useful:

1. Does the agent own something with a clear scope?
2. Does the work have its own rhythm or recurring decisions?
3. Would keeping its history avoid repeatedly explaining the same context?

A separate agent can also earn its place through sustained attention, a different viewpoint or productive discussion with a peer. Domain ownership is one pattern; specialist roles and small research pairs can fit other needs.

For example, a product organization could have a lead and owners for authentication, billing and documentation. A reviewer could provide an independent perspective across those areas. Choose the structure that makes responsibilities and handoffs understandable.

## Write the working rules

Describe how agents begin work, check evidence, exchange results and handle a blocker. Include what they should do when idle; background sessions do not create a useful work queue by themselves.

Set runtime permissions and instruct agents to call `request_approval` for actions that need your decision. [Human-in-the-loop approvals](/concepts/hitl/) record explicit requests; they do not intercept every tool call or provide a universal sandbox.

The host must stay awake and sessions must remain running, with provider access available. Separate agents keep separate context; messages connect them without merging their histories.

## Run, observe and adjust

Use [guided setup](/guides/first-team/) to discuss a team, or adapt an [example definition](https://github.com/Alireza29675/teamctl/tree/main/examples). Review the YAML and role files, validate them, then start the team.

Observe a real interaction. Check whether each agent knows its responsibility, whether messages reach the right people and whether handoffs produce useful work. Tighten the roles or communication scope before adding more agents.

## Share the definition

Share the intended YAML, role files and any required capability files. Document prerequisites and local paths so someone else can run the team and adapt it.

Each person supplies their own accounts, tools and permissions. A shared definition does not transfer live sessions, private messages or credentials. Keep `.env` and runtime state out of the shared files.
