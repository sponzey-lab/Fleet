# Sponzey Fleet Retention Policy

This document records MVP retention defaults. Sponzey Fleet is not an observability time-series platform, so stored operational artifacts must have bounded retention.

## Defaults

| Artifact | Default retention | Notes |
| --- | ---: | --- |
| Job output chunks | 14 days | Command stdout/stderr is stored as job output, not application logs. |
| Log stream artifacts | 24 hours | MVP log tail is an operator diagnostic surface, not log archival storage. |
| Metrics snapshots | 7 days | Metrics are lightweight operational snapshots, not long-term telemetry. |
| Facts snapshots | 30 days | Facts are inventory state and change less frequently than metrics. |
| Audit events | Append-only, no automatic deletion in MVP | Audit retention must be handled by an explicit operator policy after MVP. |

## Cleanup Paths

MVP includes an explicit cleanup command and a controller-managed background
worker. Both paths use the same application retention use case.

```bash
fleet retention cleanup --data-dir .sponzey --older-than-days 30 --dry-run
fleet retention cleanup --data-dir .sponzey --older-than-days 30
```

The explicit command:

- support dry-run mode before deletion,
- write an audit event for cleanup execution,
- never delete audit events by default,
- use explicit retention settings passed at command/bootstrap time,
- avoid runtime environment mutation,
- keep Product application logs free of deleted artifact bodies.

The controller worker:

- runs from controller bootstrap with code-default MVP retention durations,
- does not read or mutate runtime environment values inside the worker loop,
- logs Product-level cleanup summary counts only,
- writes the same audit event shape as the explicit cleanup command,
- never deletes audit events by default.

Both paths clean bounded operational artifact tables:

- `job_output_chunks`
- `facts_snapshots`
- `metrics_snapshots`
- `agent_log_chunks`

## Current MVP Limits

- There is no retention configuration endpoint.
- Multi-controller retention lease/leader election is not implemented.
- Agent operational log chunks stored in `agent_log_chunks` are cleaned by both retention paths.
- Raw remote file tail and journald stream archival are not persisted separately in MVP.
