# Sponzey Fleet Current Release Notes

This document captures the current post-MVP state and known limits.

## Included

- Rust workspace with layered crate boundaries.
- Single Rust `sponzey` CLI binary.
- Controller initialization with Ed25519 identity and one-time admin token output.
- Enrollment token create/list/revoke API.
- Agent enrollment with controller fingerprint pinning.
- Authenticated outbound persistent WebSocket session.
- Heartbeat, facts, metrics, operational log upload, and task dispatch are separate intervals/flows.
- Controller-signed command task envelope.
- Agent-side signature, expiry, replay nonce, and target validation.
- High-risk command confirmation boundary.
- Command output storage separated from application logs.
- Facts, metrics, and drift snapshot storage with cursor paging API and agent/controller time fields.
- Agent inventory and label update API.
- Controller-served static `/admin` Web Admin UI.
- MVP runbook parser, `sponzey apply` validation-only command, and signed controller-to-agent runbook dispatch API.
- Explicit retention cleanup command for bounded job output, facts, and metrics storage.
- Local file log tail with redaction, follow mode, max-duration guard, and journald shortcut skeleton.
- Local policy drift check engine for service running, package present, and file SHA-256 checks with signed drift job dispatch.
- Web Admin UI can select an agent, create a confirmed high-risk command job, and view polling-based job output.
- npm package wrapper for Rust binary distribution.
- Standalone release tarball packaging with `SHA256SUMS` and local verification script.
- Linux systemd install/start/status/log/uninstall commands with dry-run rendering.
- `sponzey upgrade --dry-run` policy inspection for external package/artifact upgrades.
- `sponzey demo` local loopback demo through the npm wrapper.
- Local MVP smoke script.
- Immediate dispatch and remote TLS loopback smoke scripts.
- Hardening audit script.

## Known Limits

- Controller HTTP/WebSocket serving uses Axum.
- HTTPS is supported through built-in TLS or reverse proxy deployment. HTTP remains allowed for tests only and emits warnings.
- No admin token CLI profile storage yet.
- Agent command execution streams output chunks before process completion; Web Admin UI uses polling/storage.
- Web Admin UI covers agent inventory, command job creation, output, facts, metrics, drift, jobs, and audit.
- No Ansible compatibility layer.
- `sponzey apply` validates only. Package/service/file primitive execution requires controller-signed runbook dispatch, high-risk confirmation, and an enrolled agent.
- Systemd install/start commands are implemented for Linux root environments; reboot verification remains manual.
- Automatic self-upgrade, `.deb`, `.rpm`, Homebrew, Docker, Windows service packaging, and release signature verification are not implemented yet.
- No background retention cleanup worker yet.
- No production key rotation flow yet.
- Approval request lifecycle APIs exist for approve/reject/expire. Dedicated Web Admin approval queue and RBAC/admin identity separation remain follow-up areas.
- Assignment ack/start/reject protocol states are implemented; job cancel and command timeout now produce separate `canceled` and `expired` terminal states. Multi-agent fanout aggregation remains a follow-up area.

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
