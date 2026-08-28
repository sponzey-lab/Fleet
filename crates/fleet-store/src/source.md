# Fleet Store Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Infrastructure library | Implements SQLite and feature-gated Postgres repositories, migrations, atomic verified-drift/remediation-execution/remediation-verification creation and evidence-resolution persistence, bounded recovery queries, retention, and local artifact persistence. | Performs database and filesystem I/O behind application repository contracts. |
