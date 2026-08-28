# Fleet CLI Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `main.rs` | Binary entrypoint | Starts the `fleet` command-line program. | Delegates process execution to the CLI library. |
| `lib.rs` | CLI and agent runtime | Implements command UX, bootstrap configuration, persisted remediation lifecycle rendering, deprecated manual lifecycle warnings, and the local agent socket/task adapter. | Performs filesystem, process, HTTP/WebSocket, and structured logging I/O; the `fleet-agent` library owns reconnect-surviving in-memory session/outbox state. |
