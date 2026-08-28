# Fleet Application Source Index

`fleet-application` owns use-case orchestration and repository contracts between the domain and interface/infrastructure layers.

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Application library | Defines use cases, typed repository ports, verified drift validation, and composite proposal/execution/verification creation, recovery, and evidence-resolution persistence contracts. | Performs no direct I/O; callers provide repository, audit, signer, clock, and dispatcher adapters. |
