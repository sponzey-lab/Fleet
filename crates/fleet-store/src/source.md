# Fleet Store Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Infrastructure library | Implements SQLite and feature-gated Postgres repositories, including indexed stable cursor/detail queries for durable catalog source/revision/document provenance, catalog Runbook Job and ownership-safe Policy provenance schema, source-local catalog document-body reuse, source-scoped sync-operation persistence, migrations, atomic verified-drift/remediation creation, recovery queries, retention, and local artifact persistence. | Performs database and filesystem I/O behind application repository contracts. |
