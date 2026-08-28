# Fleet Controller Source Index

`fleet-controller` owns the HTTP/WebSocket controller interface and its adapters to application and storage contracts.

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Interface library | Serves controller APIs and authenticated agent sessions, validates inbound drift correlation, renders remediation lifecycle from persisted assignment/evidence rows, rejects deprecated manual lifecycle mutations, joins persisted verification result/evidence for origin-only remediation resolution, maps task lifecycle events to application persistence ports, performs bounded pre-listener verification recovery, and dispatches committed remediation verification Jobs. | Performs network, storage, signing, audit, and session-registry coordination through explicit controller dependencies. |
