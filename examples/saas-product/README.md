# Example: saas-product

A seven-agent team running a small SaaS the way the methodology page says to: by domain ownership, not by job function. A **product_lead** agent on top holds the roadmap and talks to the operator. Six domain owners each own one product surface end-to-end. Platform owns the foundations every other domain pulls from. Auth, billing, dashboards, the docs site, and community each own the surface their name says.

This is the SaaS team from the worked example in [How to think about agent teams](https://teamctl.run/concepts/teams/), shipped as a real config. If you read the methodology and wondered *"what does that actually look like?"*, this is the answer.

```
                       product_lead (Claude Opus)              ← Telegram: product_lead bot
                          owns: roadmap, coordination, releases
                                  │
       ┌────────────┬────────────┬┴────────────┬────────────┬────────────┐
       │            │            │             │            │            │
  ┌────▼─────┐ ┌────▼───┐ ┌──────▼──┐ ┌────────▼─┐ ┌────────▼┐ ┌─────────▼─┐
  │ platform │ │  auth  │ │ billing │ │dashboards│ │docs-site│ │ community │
  │  (Opus)  │ │ (Opus) │ │  (Opus) │ │ (Sonnet) │ │(Sonnet) │ │ (Sonnet)  │
  └──────────┘ └────────┘ └─────────┘ └──────────┘ └─────────┘ └───────────┘
```

Engineering domains (platform, auth, billing, dashboards) talk on `#eng`. Outward-facing domains (docs-site, community) talk on `#external`. Product_lead sits on both, and broadcasts to `#all` when something ships.

## Why seven agents earns more than three

The SaaS team passes both gates with multiple triggers across multiple surfaces. That's what makes a seven-agent team better than collapsing the engineering domains into one "engineer agent" or the customer-facing domains into one "GTM agent".

- **Entry conditions.** Each domain owner has a *thing* with its own state, history, and decisions that compound. Auth's user model. Billing's plan structure. Dashboards' query layer. Platform's design system and shared components. Docs-site's information architecture. Community's feedback pulse. All six accumulate context over weeks and months.
- **Work-shape triggers.** *Domain separation* and *focus separation* both fire hard. Six different domains, six different rhythms, six surfaces with their own state.
- **Team-shape triggers.** *Multiple opinions* and *synergy*. When auth's new session model affects billing's webhook timing, the cross-domain conversation happens in `#eng`, not inside one agent's head. When platform ships a design-system change, every UI-consuming domain absorbs the cascade through `#eng` rather than after the fact. The team is greater than the sum because each domain's accrued context informs the others.

## What's NOT in this team (deliberately)

No PM agent. No QA agent. No engineering-manager agent. The product_lead doesn't *do* PM work. Product_lead holds the roadmap because every domain owner is the PM for their own surface. Domain owners QA their own work. There's no separate "tech writer"; docs-site owns docs as a domain in its own right.

If you found yourself reaching for a *"product manager agent"*, that's function-cut. The PM work is distributed across the people who own the things being PM'd.

## Install

```bash
# 1. Install teamctl + Claude Code.
curl -fsSL https://teamctl.run/install | sh
npm i -g @anthropic-ai/claude-code

# 2. Create one Telegram bot via @BotFather (for product_lead).
#    Get your chat id from @userinfobot.

# 3. Copy this example somewhere writable.
cp -r /path/to/teamctl/examples/saas-product ~/saas-team
cd ~/saas-team

# 4. Fill in token + chat id.
cp .team/.env.example .team/.env
$EDITOR .team/.env

# 5. Workspace. The team reads your actual product repo from here.
mkdir -p workspace
# Tip: symlink your product repo into ./workspace/ so the agents can read it.
```

## Run

```bash
set -a; . ./.team/.env; set +a

teamctl validate
teamctl up
teamctl status
```

Or use `teamctl bot setup` for the guided Telegram wizard.

## Shape of a typical week

1. Monday. Product_lead DMs you the week's picture: what each domain is in flight on, what's queued, anything cross-domain that needs your call. You redirect priorities; product_lead routes to domain owners.
2. Mid-week. Auth and billing coordinate on a session-revoke webhook in `#eng`. Platform ships a new error-boundary primitive and surfaces the cascade to dashboards (the main consumer). Dashboards picks it up in their next query layer change. None of this hits your Telegram.
3. Customer pings community with a recurring question. Community DMs docs-site, who drafts a docs update and proposes publish to you.
4. Friday. Product_lead bundles the week's shipped work into a release. `request_approval(action="release")` lands in your Telegram with the operator-facing summary. Tap ✅. Product_lead broadcasts the release to `#all`. Community drafts the changelog announcement.

[Screenshot of the product_lead bot's Monday picture: 3-5 bullets of cross-domain status]

[Screenshot of a release-approval prompt: bundle of shipped work, the operator's tap is the receipt]

## What this teaches

Three patterns layer:

1. **Domain ownership end-to-end.** Each domain owner does their own design, code, QA, and docs (in-codebase). They don't hand off to a "QA agent" or "docs agent." Docs-site exists as its own domain because the *public docs site* has its own state and lifecycle, not because someone has to write the docs for the other domains.
2. **Platform is a domain, not a layer of management.** Platform serves the other domains by owning the foundations they all pull from (design system, shared components, build pipeline). Cascades from platform changes ripple through `#eng` *before* they ship, not after. Domain ownership applies to infrastructure too.
3. **Channel design as information architecture.** `#eng` and `#external` aren't org-chart artifacts; they're how domains coordinate without flooding. Auth doesn't need to see community's daily ticket pulse; community doesn't need to read auth's session-model design discussion. Each domain sees what it needs.

## Teardown

```bash
teamctl down
rm -rf state/
```

## Related

- [How to think about agent teams](https://teamctl.run/concepts/teams/). The methodology this example is built on; this team is the worked example from that page.
- [Market analysis](../market-analysis/). Medium team where dissent is the load-bearing trigger.
- [Job finder](../job-finder/). Small team for a specific operator-facing workflow.
- [Personal research](../personal-research/). Two-agent team for a personal information loop.
