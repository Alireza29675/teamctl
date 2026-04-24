# Reference: `team-compose.yaml`

> Stub. Populated alongside Phase 1. The authoritative schema lives in `schemas/team-compose.schema.json` once Phase 1 lands.

A compose tree has two layers:

- One **global** file (`team-compose.yaml`): broker, budget, HITL policy, bot config, list of projects.
- One **per-project** file (`projects/<id>.yaml`): channels, managers, workers.

See [SPEC §5](../../../memory/lab/agent-team-orchestration/SPEC.md) (not included in this repo) for the v0.2 field list. That spec will migrate into this page during Phase 1.
