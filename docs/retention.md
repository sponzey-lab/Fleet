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

## Cleanup Command

MVP includes an explicit cleanup command:

```bash
sponzey retention cleanup --data-dir .sponzey --older-than-days 30 --dry-run
sponzey retention cleanup --data-dir .sponzey --older-than-days 30
```

The command:

- support dry-run mode before deletion,
- write an audit event for cleanup execution,
- never delete audit events by default,
- use explicit retention settings passed at command/bootstrap time,
- avoid runtime environment mutation,
- keep Product application logs free of deleted artifact bodies.

Currently it cleans bounded operational artifact tables:

- `job_output_chunks`
- `facts_snapshots`
- `metrics_snapshots`
- `agent_log_chunks`

## Current MVP Limits

- There is no background cleanup worker.
- There is no retention configuration endpoint.
- Agent operational log chunks stored in `agent_log_chunks` are cleaned by the explicit cleanup command.
- Raw remote file tail and journald stream archival are not persisted separately in MVP.
