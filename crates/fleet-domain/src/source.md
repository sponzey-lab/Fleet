# Fleet Domain Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `agent.rs` | Domain model | Defines agent identity, labels, lifecycle, and runtime profile rules. | Pure validation and state transitions; no I/O. |
| `artifact.rs` | Domain model | Defines artifact identity, retention classification, and metadata validation. | Pure data validation; no storage access. |
| `audit.rs` | Domain model | Defines auditable actors, targets, values, and events. | Pure event construction; no log or database writes. |
| `capability.rs` | Domain model | Defines agent capability snapshots and compatibility decisions. | Pure policy evaluation; no system probes. |
| `certificate.rs` | Domain model | Defines certificate lifecycle and revocation state. | Pure state transitions; no TLS or filesystem access. |
| `job.rs` | Domain model | Defines jobs, assignments, task envelopes, and execution state transitions. | Pure execution policy; no dispatch or process I/O. |
| `policy.rs` | Domain model | Defines policies, drift reports, verified remediation-origin candidate validation, and remediation rules. | Pure parsing and policy decisions; no persistence or transport access. |
| `runbook.rs` | Domain model | Defines runbook task structures and validation. | Pure parsing and validation; no task execution. |
| `secret.rs` | Domain model | Defines secret references and safe template boundaries. | Pure reference validation; never resolves secret material. |
| `selector.rs` | Domain model | Defines agent selector parsing and matching. | Pure matching; no repository access. |
| `signing.rs` | Domain model | Defines controller signing trust and rotation state. | Pure cryptographic policy state; no key-store I/O. |
| `lib.rs` | Crate surface | Exposes domain modules and shared domain identity. | No external side effects. |
