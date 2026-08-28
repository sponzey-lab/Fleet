# Fleet Agent Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Application | Owns the process-local agent connection, active-task, and bounded report-outbox state. | Pure in-memory state only; the CLI adapter owns socket I/O and process execution. |
