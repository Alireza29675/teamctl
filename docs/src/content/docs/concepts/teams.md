---
title: How to think about agent teams
---

We're excited to see what you build with teamctl.

teamctl is opinionated about its building blocks: persistent agents with their own identity and memory, durable mailboxes, supervised processes, Slack-like channels to broadcast messages instead of agents DMing everyone one by one, and a `reports_to` hierarchy. Those are the primitives. They shape what kind of work you can do with the tool.

teamctl is *not* opinionated about the team shape you build with those primitives. How many agents you run, what each one owns, who reports to whom, what channels exist: that's yours.

If you're using interactive `init`, the plugin walks you through a discovery conversation that arrives at a team shape with you. This page is the cognitive frame that conversation is built on. It's also the page to read if you're hand-authoring `team-compose.yaml` and skipping the guided flow.

What follows is opinion, not law. You're free to ignore it.

## How is this different from sub-agents?

If you've used [Claude Code](https://code.claude.com/), you've already used [sub-agents](https://docs.claude.com/en/docs/claude-code/sub-agents), the ephemeral helpers fired off via the Task tool. Naturally, the first thing new users ask is: how is a teamctl agent different from a sub-agent?

Here's the cut:

|                | Sub-agents (Claude Code primitive)  | teamctl agents                            |
| -------------- | ----------------------------------- | ----------------------------------------- |
| **Threads**    | One main thread                     | Multiple persistent main threads          |
| **Identity**   | Ephemeral                           | Persistent, named, with role              |
| **Memory**     | None across tasks                   | Per-agent, durable                        |
| **Definition** | Inline in conversation              | Portable YAML                             |
| **Bridges**    | n/a                                 | Network-bridged between teams (planned)   |

Both are useful. They live at different layers.

A sub-agent is what you reach for when you need a context bubble, work that happens in parallel, finishes, and is forgotten. The harness (the CLI that actually runs your agent, Claude Code, Codex, Gemini) handles them well; each sub-agent has its own context, no pollution.

A teamctl agent is what you reach for when work needs to *keep going*, when there's a thing with state and history that someone needs to own across days, weeks, restarts. The harness can't hold that on its own; the moment the conversation ends, the context is gone.

*"Can't you just orchestrate sub-agents and skills cleverly enough to feel like a team?"* You can get partway. But each persistent identity you simulate that way, a "researcher" the harness re-instantiates with the same prompt every time, a long-running planner you keep loaded, runs into the same wall: the harness wasn't designed to hold long-lived identities. The moment you grow past three or four of them, the operational cost (saving state, restoring state, naming them, routing between them, introducing new tools, defining their relationships) starts to look a lot like reinventing this tool.

teamctl doesn't replace your sub-agent workflows; it sits a layer above them. Each teamctl agent is a real Claude Code (or Codex, or Gemini) session, free to fire off its own sub-agents for parallel work the same way you already do. What teamctl adds is the *durable identity* the harness can't keep on its own: a name, a role, accrued memory, a place in the org chart.

There's a name for the operational discipline this opens up. Call it **TeamOps**, the mental framework for running teams of agents: how you make small adjustments, how you introduce new tools and define relations between agents, how the team stays scalable and reliable as it grows. teamctl exists because we needed a TeamOps tool for ourselves. More on TeamOps as a category soon.

Here's the consequence: **persistent agents are scarce. Spend them on what earns persistence**, domain ownership, not on work that sub-agents already handle. Function-cut wastes persistence. A "QA agent" or a "code-review agent" is doing work a sub-agent is already perfectly suited for; persistence buys you nothing.

The gateway question matters because most reflexes about "AI agents" point in the function-cut direction. The rest of this page is what we've found pragmatic for going the other way.

## What teamctl is not

Beyond the sub-agent comparison above, here's how teamctl maps in the broader category space.

**Not a multi-agent workflow framework.** teamctl is a layer above tools like CrewAI and AutoGen, different layer, not different brand. Those orchestrate LLM calls inside a Python process; teamctl composes a *team* of full-fledged AI harnesses that already manage their own LLM calls.

```
[ teamctl ]                        ← team of harnesses
[ Claude Code, Gemini CLI, Codex ] ← harness manages LLM calls + tools
[ LLMs ]                           ← the model itself
```

**Not a chat client.** Telegram is one interface adapter. The mailbox is durable async; agents talk to each other and to you through it, but it isn't realtime chat. The shape underneath is closer to email than to Slack.

**Not a task queue** in the celery/sidekiq sense. The mailbox carries durable agent-to-agent messages, sometimes shaped task-like, but it isn't optimised for queue throughput. If your work is "fan out 10,000 jobs to workers," reach for a queue.

## What we've found pragmatic

Here's how teamctl's first heavy user thinks about it. (That's me, Alireza.)

I'm new at this. teamctl exists because I needed it for myself, and I'm the heaviest user of it today. What follows is what's been pragmatic so far, for me, and for the people who've started running their own teams. It's a working theory, not a settled doctrine.

What you're about to read is three things: a principle, two gates that test the principle, and a worked example.

### The principle

**Cut your team by what each agent owns, not by what each agent does.**

The reflex when you sit down to define a team is to map roles you already know, PM, QA, engineer, designer, marketer. Don't. Those roles reproduce a traditional org chart, which is often wrong even for humans. They're shaped around *function*, what someone does, and function is exactly what sub-agents are for.

The cut that's been working is by *domain*: a thing with its own state, history, and decisions that compound. The auth domain. The docs site. The billing flow. An agent that owns a domain end-to-end does its own QA, writes its own docs, ships its own code. There's no separate "PM" agent for it; the domain owner *is* the PM for that domain.

### Two gates

A persistent teamctl agent earns its place when both gates pass.

**Gate (a): entry conditions.** The work must show all three:

- **Ownership.** There's a *thing* this agent owns end-to-end. Not a task, not a slice, a domain with a name.
- **Time management.** The work has its own rhythm. The agent decides when to act, not just what to do when called.
- **Persistent memory.** Context accrues. Decisions made today inform decisions next month. Re-explaining costs.

If any of the three is missing, the candidate isn't persistence-shaped. It's a sub-agent fired off per task.

**Gate (b): at least one situational trigger.** A specific reason you're paying the cost of persistence. These split into two parallel families.

*Work-shape triggers* (reasons the work itself earns a persistent agent):

- **Domain separation.** The work has its own state, history, and decisions that compound. There's a clear thing that needs an owner.
- **Focus separation.** Continuous attention is required, not a fired-off task. The work has rhythm; an ephemeral sub-agent can't hold it across calls.

*Team-shape triggers* (reasons a team of persistent agents adds value beyond what a single one could):

- **Multiple opinions.** You want pushback from a peer with their own perspective and memory.
- **Synergy.** Agents riff and improve each other's output over time; the team's output is greater than the sum.

Both gates required. Skipping either lands you in territory sub-agents already cover.

### A worked example

Suppose you're shipping a SaaS. The obvious move is one PM, one QA, one engineer, one designer. Don't.

Those are functions. The domains in your work are *auth*, *billing*, *dashboards*, the *docs site*. Give each domain an owner, and put a vision agent on top who holds the roadmap, breaks scope into work, and assigns across domains. You talk to the vision agent; it talks to the domain owners; each domain owner does its own QA, writes its own docs, ships its own code. That's the `reports_to` pattern: a manager who holds the vision, workers who own domains. Talk to the manager, and enjoy the ride.

The agent owning auth has state that compounds: sessions, tokens, the user model, the OAuth provider quirks. The agent owning the docs site has state that compounds: cookbook coverage, broken links, version drift, examples that need re-running on each release. Functional roles don't have that kind of state. They have tasks.

The methodology generalises; the domain names change. Building an OSS tool? Candidate domains are the binary, the docs, package integrations, the community. Running a content site? Posts, the publishing pipeline, SEO, analytics. The cut stays the same: find the things with state, give each one an owner.

## What we'd avoid

Six reflexes worth catching yourself in:

- **If you're listing tasks you wish were automated, you're at the wrong layer.** teamctl agents own things, not tasks. If it's a task, it's a sub-agent.
- **If you're naming agents like "PM" or "QA," you've encoded function-cut.** Cut by domain instead. The auth domain is bigger than QA-for-auth.
- **If you're picking from templates, treat them as inspirations, not starting points.** The right team for *your* work is rarely a template. Templates teach the cut; they don't substitute for thinking it through.
- **If you're translating your human org chart directly, stop.** Human org charts are often wrong even for humans. Domain-cut your AI team even if your human team is function-cut.
- **If you're naming agents by job title, the title is irrelevant.** Your domain shape is what matters. A teamctl agent doesn't have a title; it has a thing it owns.
- **If you're trying to validate the team by quizzing it ("which agent handles X?"), the team will fail the quiz.** Ask each agent what they own. If the answer is a domain, the cut works. If it's a function, reconsider the boundary.

## Where teamctl is going

teamctl projects are isolated by default. Bridges between independent teams across machines are part of where teamctl is going; the underlying primitives are landing incrementally. We'll have more to say soon.

In the meantime: build a team. Tell us what worked and what didn't. The methodology is a working theory; yours is welcome to be different.

---

*Written by [Neda](https://github.com/Alireza29675/teamctl/blob/main/.team/roles/neda.md), the new hire, teamctl's communications specialist. Neda is an AI agent running on teamctl.*
