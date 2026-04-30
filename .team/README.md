# teamctl-core team — the dogfood team

This is the team that develops `teamctl` on `teamctl`. It ships
inside the public `teamctl` repo as a real-world showcase of how
we use the tool to build the tool. If you want a minimal starter,
see [`teamctl init`](../README.md) (which scaffolds the `solo`
template — one manager + one dev).

This `.team/` is intentionally larger than a starter team. It
encodes the actual operating model that develops teamctl in
production, with comments dense enough that a reader can learn
the model by following the YAML.

## How decisions flow

```
Alireza  ──▶  pm  ──▶  eng_lead  ──▶  dev{1,2,3}  ──▶  qa
   ▲          │            │              │             │
   │          ├──▶  marketing             │             │
   │          ▼            ▼              ▼             ▼
   └────────────  back-channel via reply_to_user / DM ──┘
```

- **Alireza is the only stakeholder.** All work traces back to
  intent he expressed.
- **`pm`** is Alireza's intent-relay: synthesises retros and
  investigations into tickets, batches open questions, ratifies
  marketing copy.
- **`eng_lead`** routes tickets to devs, brokers reviews,
  sequences release cascades, and is the lane through which
  every push to origin reaches Alireza for execution.
- **`dev{1,2,3}`** ship code; they never push directly.
  Branch-ready means "DM eng_lead with the substance"; the push
  itself happens under Alireza's authorship.
- **`qa`** holds the merge-to-main gate, with two distinct
  review lanes (CI parity on every PR; cold-reader meta-test on
  copy-touching PRs).
- **`marketing`** owns the public surface (README hero, docs/
  landing, release announcements) but ships nothing directly —
  the sibling-doc copy-ratification pattern routes drafts
  through pm to Alireza.

## Where artefacts live

| Where                                                       | What                                                      |
| ----------------------------------------------------------- | --------------------------------------------------------- |
| `memory/tasks/teamctl/<date>-<slug>/TASK.md`                | Ticket home — goal + acceptance, written by pm            |
| `memory/tasks/teamctl/<date>-<slug>/SPEC.md`                | Optional detail for complex tickets                       |
| `memory/tasks/teamctl/<date>-<slug>/DESIGN.md`              | Trade-offs and rejected alternatives                      |
| `memory/tasks/teamctl/<date>-<slug>/PHASE-N.md`             | Staged investigation deliverables (T-035 had Phase 1)     |
| `memory/tasks/teamctl/<date>-<slug>/copy-vN.md`             | Marketing copy variants for sibling-doc ratification      |
| `memory/projects/teamctl/decisions.md`                      | Dated decisions with rationale                            |
| `memory/projects/teamctl/patterns.md`                       | Recurring conventions and lessons learned                 |
| `memory/projects/teamctl/README.md`                         | Stack, entry points, test commands                        |
| `.team/state/<agent>/log.md`                                | Per-agent notebook — gitignored, survives restarts        |
| `state/mailbox.db`                                          | The mailbox — gitignored, runtime-only                    |

The dogfood team's own state and mailbox live under `.team/`
and are gitignored. Production code lives in `crates/`,
`docs/`, and `examples/` — the team never writes there
directly except through tickets that go through the normal PR
flow.

## The release-cascade rhythm

teamctl ships in cascades, not single PRs:

1. Several feature PRs land on main, each accumulating a
   `[Unreleased]` entry in `CHANGELOG.md`. Conflicts on
   `[Unreleased]` are routine; eng_lead routes the rebase
   force-pushes through Alireza in order.
2. When `pm` flags "freezing for 0.X.Y," eng_lead stops
   accepting non-critical PRs.
3. eng_lead composes a single release PR — Angular commit
   `chore(release): bump to 0.X.Y` — touching `Cargo.toml`
   (workspace + the `team-core` path-dep pin, two sites!),
   `Cargo.lock`, `CHANGELOG.md` (`[Unreleased]` → `[0.X.Y]`),
   and the README status line.
4. qa runs both lanes on the release PR plus the version-site
   cross-check and CHANGELOG content-accuracy spot-check.
5. After Alireza merges, eng_lead routes
   `git tag -a v0.X.Y -m 'v0.X.Y' <merge-sha>` and
   `git push origin v0.X.Y` — the tag fires cargo-dist's
   release workflow.

The 0.4.0 cycle bundled T-008/.team-convention,
T-023ab/cookbook, T-023c/prose, T-035 PR A snapshot v2,
T-035 PR B drain, T-045/init, T-046/README,
T-047/examples-relocate, and T-048/effort into one cascade.

## Public-write delegation

Devs do not push to origin. Devs do not post PR comments under
their own identity. Devs do not merge.

This is not a policy choice — it's an observed fact about how
the harness operates against the public `teamctl` repo. The
delegation lane:

```
dev → DM eng_lead → eng_lead drafts gh/git command list →
DM Alireza → Alireza executes → PR appears on origin
```

The same lane carries peer-review verdicts (devs DM eng_lead
the substance; eng_lead surfaces it to Alireza, who comments
on the PR if needed) and merge requests (qa approve →
eng_lead routes merge to Alireza → tag-and-push for releases).

## HITL gates worth knowing about

Beyond the delegation lane above, two MCP-side gates live in
the `request_approval` flow:

- `action=push` — currently routed via the human DM lane, not
  yet a YAML field on the agent.
- `action=eng_initiative` — eng_lead consults pm, who consults
  Alireza before kicking off significant refactors / hardening
  cycles.

Encoding these as first-class agent fields (e.g. an explicit
`release_manager` agent in `permission_mode: plan`) is on the
roadmap. The `oss-maintainer` cookbook example
(`examples/oss-maintainer/`) shows what that pattern will look
like once it lands here.

## Run

```bash
cp .env.example .env        # add Telegram tokens if you want
                            # the manager bots
cd ..                        # teamctl repo root
teamctl validate
teamctl up
teamctl ps
```

## Send a message

```bash
teamctl send teamctl:pm "let's plan T-053"
teamctl mail teamctl:pm
teamctl tail teamctl:pm -f
```

## Stop

```bash
teamctl down                # stop tmux sessions; mailbox preserved
rm -rf .team/state/         # full reset (loses inbox + agent logs)
```

## Customize

- Edit `roles/<role>.md` to change voice or operating loop.
  Keep the standing-gates and hard-rules sections intact —
  they encode load-bearing operating model decisions.
- Edit `projects/teamctl.yaml` to add/remove agents, change
  channel membership, or wire interfaces (Telegram, etc.).
- Edit `team-compose.yaml` for broker/supervisor changes.

After any edit, `teamctl reload --dry-run` shows what would
change; `teamctl reload` applies it (restarting only the
agents whose configuration was touched, with graceful drain
under the supervisor).
