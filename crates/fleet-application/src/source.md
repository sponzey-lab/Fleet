# Fleet Application Source Index

`fleet-application` owns use-case orchestration and repository contracts between the domain and interface/infrastructure layers.

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Application library | Defines use cases and typed repository ports, including durable catalog source/revision/document cursor/detail reads, catalog registration, unchanged-commit/document reuse during sync reduction, active ready catalog Runbook snapshots into signed approval-governed Jobs, ownership-safe catalog Policy publication, controller-owned fetch cancellation signal, activation, verified drift validation, and composite remediation persistence contracts. | Performs no direct I/O; callers provide repository, audit, signer, clock, fetcher, and dispatcher adapters. |
