# Sponzey Fleet Current Release Notes

This document captures the current post-MVP state and known limits.

## Included

- Rust workspace with layered crate boundaries.
- Single Rust `fleet` CLI binary.
- Controller initialization with Ed25519 identity and one-time admin token output.
- Enrollment token create/list/revoke API.
- Agent enrollment with controller fingerprint pinning.
- Authenticated outbound persistent WebSocket session.
- Heartbeat, facts, metrics, operational log upload, and task dispatch are separate intervals/flows.
- Controller-signed command task envelope.
- Agent-side signature, expiry, replay nonce, and target validation.
- Agent local file-backed replay nonce store rejects consumed task nonce after restart and fails closed if the store is unreadable or corrupt.
- High-risk command confirmation boundary.
- Approval request lifecycle with approve/reject/expire APIs and Web Admin queue.
- Selector preview, target snapshot storage, assignment state tracking, fanout concurrency, maxFailures, and partial-success aggregation.
- Command output storage separated from application logs.
- Facts, metrics, and drift snapshot storage with cursor paging API and agent/controller time fields.
- Agent inventory and label update API.
- Controller-served static `/admin` Web Admin UI.
- MVP runbook parser, `fleet apply` validation-only command, and signed controller-to-agent runbook dispatch API.
- Explicit retention cleanup command and controller-managed retention worker for bounded job output, facts, metrics, and agent log storage.
- Local file log tail with redaction, follow mode, max-duration guard, and journald shortcut skeleton.
- Local policy drift check engine for service running, package present, and strict 64-character lowercase file SHA-256 checks with signed drift job dispatch.
- Agent capability snapshot reporting, SQLite persistence, Agent API/Web Admin summary, and dispatch-time unsupported capability rejection.
- Web Admin UI can inspect agents, reported capabilities, facts, metrics, logs, drift latest/history, jobs, target assignments, audit events, enrollment tokens, approvals, runbook jobs, direct policy assignment, and selector target previews for command/runbook job creation.
- Audit export API and `fleet audit export` CLI with category filtering, cursor paging, JSONL output, and SecretRef marker redaction.
- npm package wrapper for Rust binary distribution.
- Standalone release tarball packaging with `SHA256SUMS`, checksum verification, and release checksum signature sign/verify scripts.
- Linux systemd install/start/status/log/uninstall commands with dry-run rendering.
- `fleet upgrade --dry-run` policy inspection for external package/artifact upgrades.
- `fleet demo` local loopback demo through the npm wrapper.
- Local MVP smoke script.
- Immediate dispatch and remote TLS loopback smoke scripts.
- Hardening audit script.

## Known Limits And Post-MVP Phase Mapping

- Controller HTTP/WebSocket serving uses Axum.
- HTTPS is supported through built-in TLS or reverse proxy deployment. HTTP remains allowed for tests only and emits warnings.
- CLI login/profile storage exists at `.fleet/cli-profile.json` with owner-only permission checks. OIDC/SSO, API token lifecycle, project/team RBAC, and full multi-admin sessions are Phase 6 work.
- Agent command execution streams output chunks before process completion; Web Admin UI uses polling/storage.
- Web Admin UI covers agent inventory, command job creation, output, facts, metrics, drift, jobs, and audit.
- No Ansible compatibility layer exists. The documented subset import/report path is Phase 13 work and must not execute during import.
- `fleet apply` validates only and does not resolve `secretRefs`. Package/service/file primitive execution requires controller-signed runbook dispatch, high-risk confirmation, and an enrolled agent. Plain-variable `file.template` parser/render/runner mapping, explicit SecretProvider-compatible resolver injection, TaskResult artifact metadata reporting with optional non-secret body persistence, SQLite rendered artifact metadata storage, a local `ArtifactStore` body contract, Controller artifact retrieval API, Web Admin artifact metadata/retrieval surface, typed `SecretRef`, application `SecretProvider` contract, typed startup `SecretProviderSettings`, controller bootstrap provider factory, and agent runbook resolver handoff exist. Product provider source configuration, external provider adapters, and S3-compatible storage remain Post-MVP work.
- S3-compatible adapter decision is recorded: implementation is deferred until typed bootstrap artifact store settings, external secret reference credentials, feature-gated contract tests, and redaction tests exist.
- Systemd install/start commands are implemented for Linux root environments; reboot verification remains manual.
- Automatic self-upgrade, one-line installer, `.deb`, `.rpm`, Homebrew, Docker, and Windows service packaging are Phase 9 and Phase 12 work.
- Release checksum signature verification scripts exist for `SHA256SUMS.sig`; official release key publication and operational signing process are Phase 9 work.
- Retention worker uses code-default MVP durations and has no runtime configuration endpoint or HA lease/leader election. Cleanup keeps audit events out of normal deletion and separates job output, facts, metrics, and agent log cutoffs. HA-safe lease coordination is Phase 10 work; runtime config patching remains prohibited.
- No production controller signing key rotation, agent certificate rotation, or mTLS client-certificate lifecycle exists. These are Phase 5 work.
- Capability reporting, persistence, and basic dispatch rejection gate exist with a 24-hour stale snapshot guard. Platform-specific privilege/package/service probing and manual platform smoke are Phase 12 and Phase 15 work.
- Replay nonce protection is local and file-backed on each agent. Cross-controller/global replay correlation is not implemented; HA-safe coordination belongs to Phase 10.
- Approval request lifecycle and Web Admin approval queue exist. Product-grade multi-admin identity, sessions, and external auth are Phase 6 work.
- Assignment ack/start/reject protocol states are implemented; job cancel and command timeout now produce separate `canceled` and `expired` terminal states. Multi-agent fanout aggregation, capability-aware dispatch, queued-only dispatch claim, and send-failure release exist. Release does not requeue terminal assignments. Phase 15 must add long-running recovery smoke for reconnect, retry, timeout, and output replay cases before scale readiness is accepted.
- Scheduled drift entries are stored, queried, and consumed by a controller worker that creates signed due drift-check jobs. The worker is single-controller safe only; multi-controller lease/leader election is Phase 10 work.
- Policy-based remediation request domain/application proposal state machine, SQLite metadata persistence, persisted approval request creation, approved signed runbook job creation, task-event lifecycle reconciliation, Controller API surface, and thin CLI/Web Admin surface exist. A Controller-verified, signed, drifted policy check creates one idempotent `Proposed` request with its origin report and redacted audits in the same persistence transaction; compliant and unverified reports remain observations. A successful remediation result creates one correlated signed verification drift Job, assignment, and audit atomically, then attempts dispatch after commit; a disconnected agent remains queued. Before listener readiness, a single bounded startup scan reconciles correlation-free pending verification rows through the same create boundary; unverifiable legacy rows are audit-skipped without dispatch. A successful verification result and fresh compliant persisted evidence after remediation execution atomically resolve the remediation and only its origin drift, regardless of report/result delivery order; stale, drifted, unknown, failed, or mismatched evidence remains pending. The legacy manual running/result/verify API and CLI commands remain only as deprecated `409` compatibility endpoints; Web Admin displays persisted lifecycle state and provides no manual transition control. Execution is still approval-gated: there is no approval-bypassing auto-remediation worker. Phase 3 now has typed database backend settings, startup-only Postgres URL/SSL mode/connect timeout/pool parsing, a blocking Postgres client pool boundary, a minimum native TLS adapter for `sslmode=prefer/require`, a shared SQLite repository contract harness, a feature-gated Postgres migration skeleton with ignored integration gate, repository slices through RemediationRequest, `ControllerStore`/`ControllerStoreRef` boundaries, feature-gated Controller Postgres open/migration wiring, direct server runtime adapter dispatch, typed job+assignment transaction boundary, and queued-only dispatch claim/release contract. Custom CA/client certificate rotation, scheduled drift/retention lease, and HA claim semantics remain.
- Audit events are append-only at the controller API/application boundary and are excluded from normal retention cleanup. The SQLite MVP store is not a tamper-proof WORM audit store; tamper-evident hash chain, signed export manifest, and compliance report are Phase 11 work.
- Slack/Teams notification, Prometheus/OpenTelemetry export, and Git runbook/policy sync are Phase 8 and Phase 7 work.
- Plugin/external adapter execution is not available. Signed manifest, disabled-by-default policy, and approval/capability/audit boundaries are Phase 14 work.
- 100-agent heartbeat and 1,000-output load gates are Phase 15 work.

## Demo Safety

HTTP controller URLs are allowed for setup checks, local development, lab tests, and short-lived validation only. Product, customer, production, shared, or long-running environments must use HTTPS.

Every HTTP path prints an insecure transport warning because traffic is not encrypted. A controller configured with an HTTP external URL also writes a Security audit event. HTTP transport provides no confidentiality or integrity guarantee and can expose tokens, commands, operational data, and traffic to man-in-the-middle attacks.

## Verification

Current readiness is checked with:

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test --prefix npm/fleet
npm test --workspace @sponzey/fleet
npm run build --workspace web-admin
npm test --workspace web-admin
npm run typecheck --workspace web-admin
./scripts/npm_local_pack_smoke.sh
./scripts/npm_platform_local_install_smoke.sh
./scripts/npm_demo_smoke.sh
./scripts/smoke_mvp.sh
./scripts/smoke_immediate_dispatch.sh
./scripts/smoke_remote_tls_loopback.sh
./scripts/signature_verification_smoke.sh
./scripts/hardening_audit.sh
```

For a full local release gate:

```bash
./scripts/release_readiness_gate.sh
```

For destructive Linux checks, run on a Linux host with root privileges:

```bash
sudo ./scripts/release_readiness_gate.sh --include-manual
sudo reboot
sudo ./scripts/release_readiness_gate.sh --verify-manual-reboot
```

After npm registry publish, verify the installed wrapper with:

```bash
./scripts/release_readiness_gate.sh --include-registry
```
