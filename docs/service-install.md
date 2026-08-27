# Sponzey Fleet Service Install Notes

Service installation supports dry-run rendering everywhere and guarded systemd writes on Linux when run as root. The service entrypoint always pins the resolved absolute Rust binary path instead of relying on an npm global shim.

## Commands

```bash
sponzey controller install-service --data-dir /var/lib/sponzey-fleet --dry-run
sponzey agent install-service --data-dir /var/lib/sponzey-fleet --dry-run
sponzey controller start-service --dry-run
sponzey agent start-service --dry-run
sponzey controller restart-service --dry-run
sponzey controller status-service --dry-run
sponzey agent status-service --dry-run
sponzey controller logs-service --lines 50 --dry-run
sponzey agent logs-service --lines 50 --dry-run
sponzey controller uninstall-service --dry-run
sponzey agent uninstall-service --dry-run
```

Without `--dry-run`, `install-service` writes `/etc/systemd/system/sponzey-fleet-controller.service` or `/etc/systemd/system/sponzey-fleet-agent.service`, then runs `systemctl daemon-reload` and `systemctl enable ...`. `start-service` runs `systemctl start ...`. `restart-service` runs `systemctl restart ...` for the controller service. `status-service` runs `systemctl status ... --no-pager`. `logs-service` runs `journalctl -u ... --no-pager -n <lines>`. `uninstall-service` runs `systemctl disable --now ...`, removes the service file, then runs `systemctl daemon-reload`.

Non-Linux hosts fail with a clear Linux requirement. Install/start/uninstall
operations require root. Status/log queries require Linux/systemd but do not
require root. Dry-run never writes system files.

For controller signing key rotation, record restart intent first through
`sponzey controller signing-rotation restart-action --confirm-external-restart`,
then run `sponzey controller restart-service`. The API records audit intent but
does not self-restart the HTTP handler or reload key material in-process.

The MVP repository also provides foreground scripts for local development:

```bash
./scripts/run_controller.sh
./scripts/run_agent.sh
```

`run_agent.sh` does not auto-initialize or enroll the agent. Use the same
`--data-dir` for controller init, token creation, agent init, and agent start:

The HTTP example below is for local testing only. Product, customer,
production, shared, or long-running environments must use HTTPS.

```bash
./target/debug/sponzey controller init --data-dir .sponzey
./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir .sponzey --external-url http://127.0.0.1:7700
TOKEN=$(./target/debug/sponzey enroll-token create --data-dir .sponzey --labels role=web,env=dev)
./target/debug/sponzey agent init --data-dir .sponzey --url http://127.0.0.1:7700 --token "$TOKEN" --name web-01 --labels role=web,env=dev
./scripts/run_agent.sh --data-dir .sponzey
```

## Required Service Properties

Systemd unit generation:

- pin the resolved absolute Rust binary path,
- avoid relying on npm global shim paths for service execution,
- pass controller/agent role through explicit CLI arguments,
- pass data directory through explicit CLI arguments,
- avoid runtime environment mutation,
- fails clearly when Linux/root requirements are not met,
- supports dry-run output before writing system files.
- supports dry-run output before disabling/removing service files.
- supports status and recent journald log inspection.

## Manual Systemd Shape

Controller service direction:

```ini
[Unit]
Description=Sponzey Fleet Controller
After=network-online.target

[Service]
Type=simple
ExecStart=/absolute/path/to/sponzey controller start --data-dir /var/lib/sponzey-fleet
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Agent service direction:

```ini
[Unit]
Description=Sponzey Fleet Agent
After=network-online.target

[Service]
Type=simple
ExecStart=/absolute/path/to/sponzey agent start --data-dir /var/lib/sponzey-fleet
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

HTTP transport is test-only. Product, customer, production, shared, and long-running environments must use HTTPS.

## TLS And Signing Identity

Controller service TLS options and controller task-signing keys are separate
trust materials.

- `--tls-cert` and `--tls-key` configure only the TLS server identity used by
  HTTPS/WSS.
- Controller task-signing keys stay under the controller data directory as
  `controller/controller_public.key` and `controller/controller_private.key`.
- Do not reuse the TLS private key as the controller signing private key.
- Do not treat a TLS certificate fingerprint as the controller signing
  fingerprint pinned by agents.
- Provide `--tls-cert` and `--tls-key` together. Supplying only one is rejected
  during controller bootstrap.
- Service units must pass key paths as explicit CLI arguments or data directory
  arguments. They must not inject, change, or discover trust settings by editing
  process environment variables after startup.

Agent client certificates are not a product mTLS requirement yet. When that
feature is added, it must be configured as explicit startup trust settings and
must not bypass agent key proof or controller-signed task verification.
`--agent-client-ca-cert` is reserved for that future mTLS trust material and is
currently rejected during controller bootstrap because the built-in listener
does not enforce agent client certificates yet. The rejection must not expose
the local CA certificate path.

Controller signing key replacement is not a runtime environment edit. The
controller filesystem boundary validates staged signing files with an explicit
challenge, rejects TLS/active path reuse, requires private key file permissions
that are not group/other accessible, writes backups, replaces through temporary
files, verifies the swapped pair, and rolls back on partial failure. A later
API/CLI command may call that boundary, but service units must still pass
startup settings explicitly and restart/reload through an explicit operation.
On controller start, active signing material is validated and compared with the
persisted signing rotation state. If the selected signing fingerprint does not
match the active keypair, the controller fails closed before accepting API or
WebSocket traffic.

Agents treat the enrolled controller signing fingerprint as a public trust
anchor. Current agent builds adapt that pinned fingerprint and the controller
signing public key into a one-entry trust bundle before verifying task
envelopes. Agent sessions can accept a controller signing trust bundle update
message containing only public fingerprints, public keys, roles, and validity
windows, then store the accepted bundle in explicit in-memory session state for
subsequent task verification. Accepted updates are persisted as
`controller_trust_bundle.json` beside `agent.conf` using the same public-only
schema, so agent restart keeps the rotation trust window. Service units must
not inject trust changes by editing process environment variables while the
agent is running. Operator-facing rotation commands and controller-side rollout
scheduling are separate follow-up work.

## Upgrade Policy

`sponzey upgrade` is intentionally a dry-run planning command in the current
release line. It does not replace the running binary or edit service files.

```bash
sponzey upgrade --dry-run
sponzey upgrade --channel beta --version 0.2.0-beta.1 --dry-run
```

The supported upgrade path is:

1. Stop controller and agent services.
2. Back up controller data with `sponzey controller backup`.
3. Download an npm package update or standalone release archive.
4. Verify release artifact integrity with `SHA256SUMS` and, when published,
   `SHA256SUMS.sig`.
5. Replace the `sponzey` binary through the chosen package/artifact mechanism.
6. Start services and verify with `status-service` and `/healthz`.

If upgrade fails before controller storage migration, restore the previous
binary and restart services. If storage was migrated or modified, restore the
controller backup before restarting the controller.

Stable/beta channel policy:

- Stable releases use regular semver versions and publish npm packages with the
  `latest` tag.
- Prerelease versions publish with the npm `next` tag and are treated as beta.
- Automatic downgrade is not supported; restore the previous binary and backup
  manually when rollback is required.

Package formats not yet implemented and owning phase:

- `.deb`: Phase 9, with systemd unit render test and signature/version pinning.
- `.rpm`: Phase 9, with systemd unit render test and signature/version pinning.
- Homebrew formula: Phase 9 decision record and smoke command.
- Docker image: Phase 9 decision record, image tag/version pinning, and checksum/signature policy for embedded binary.
- Windows service package: Phase 12 agent/service support first, then Phase 9 packaging track.

Windows production service support is not currently published. Unsupported npm
platforms fail with a clear `unsupported platform` error.

One-line installer is Phase 9 work. It must provide `--dry-run`, explicit
version/channel selection, checksum verification, release signature
verification, and must not silently edit shell profiles or runtime environment
variables.

## Standalone Artifact Integrity

GitHub release artifacts use this naming rule:

```text
sponzey-darwin-arm64.tar.gz
sponzey-darwin-x64.tar.gz
sponzey-linux-arm64.tar.gz
sponzey-linux-x64.tar.gz
```

The release workflow also uploads `SHA256SUMS`. Verify downloaded artifacts:

```bash
./scripts/verify_standalone_artifacts.sh dist/release
```

The verification script checks:

- each artifact listed in `SHA256SUMS` exists,
- the SHA-256 digest matches,
- the archive extracts successfully,
- an executable `sponzey` binary is present.

Release checksum signatures cover the `SHA256SUMS` manifest. When
`SHA256SUMS.sig` is published, verify it with the pinned release public key from
the project release channel:

```bash
./scripts/verify_release_signature.sh dist/release ./release-public-key.pem
```

Maintainers sign the manifest with an explicit private key path:

```bash
./scripts/sign_release_sums.sh dist/release ./release-private-key.pem
```

The public key must not be trusted merely because it was downloaded inside an
artifact archive.

## Manual Reboot Smoke

The repository includes a guarded manual smoke script for the destructive Linux/systemd verification that cannot run in the default local suite.

Requirements:

- Linux host
- root privileges
- systemd
- built `sponzey` binary or `SPONZEY_BIN` pointing to an absolute binary

Run before reboot:

```bash
sudo ./scripts/manual_systemd_reboot_smoke.sh install
# or run it through the release gate
sudo ./scripts/release_readiness_gate.sh --include-manual
```

Then reboot the host and verify:

```bash
sudo ./scripts/manual_systemd_reboot_smoke.sh verify
# or verify through the release gate
sudo ./scripts/release_readiness_gate.sh --verify-manual-reboot
```

The script checks that both `sponzey-fleet-controller.service` and `sponzey-fleet-agent.service` are enabled and active.

## Manual npm Registry Smoke

After publishing `@sponzey/fleet` and its platform packages to the npm registry:

```bash
./scripts/npm_publish_current_platform.sh --dry-run
SPONZEY_NPM_TOKEN_FILE=token.md ./scripts/npm_publish_current_platform.sh
./scripts/manual_npm_registry_smoke.sh
# or run it through the release gate
./scripts/release_readiness_gate.sh --include-registry
```

The script installs into a temporary npm prefix and verifies that `sponzey --help` runs through the installed wrapper.

For full multi-platform npm publish, use the GitHub Actions workflow in
`.github/workflows/npm-release.yml`. Configure npm Trusted Publisher for the
wrapper and all four platform packages, bump all package versions, then push a
matching new tag. The workflow authenticates with a short-lived GitHub Actions
OIDC identity and does not require an `NPM_TOKEN` repository secret. See
[`npm-trusted-publishing.md`](npm-trusted-publishing.md) for the exact package
and publisher settings.
