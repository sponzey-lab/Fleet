# Sponzey Fleet Service Install Notes

Service installation supports dry-run rendering everywhere and guarded systemd writes on Linux when run as root. The service entrypoint always pins the resolved absolute Rust binary path instead of relying on an npm global shim.

## Commands

```bash
sponzey controller install-service --data-dir /var/lib/sponzey-fleet --dry-run
sponzey agent install-service --data-dir /var/lib/sponzey-fleet --dry-run
sponzey controller start-service --dry-run
sponzey agent start-service --dry-run
sponzey controller status-service --dry-run
sponzey agent status-service --dry-run
sponzey controller logs-service --lines 50 --dry-run
sponzey agent logs-service --lines 50 --dry-run
sponzey controller uninstall-service --dry-run
sponzey agent uninstall-service --dry-run
```

Without `--dry-run`, `install-service` writes `/etc/systemd/system/sponzey-fleet-controller.service` or `/etc/systemd/system/sponzey-fleet-agent.service`, then runs `systemctl daemon-reload` and `systemctl enable ...`. `start-service` runs `systemctl start ...`. `status-service` runs `systemctl status ... --no-pager`. `logs-service` runs `journalctl -u ... --no-pager -n <lines>`. `uninstall-service` runs `systemctl disable --now ...`, removes the service file, then runs `systemctl daemon-reload`.

Non-Linux hosts fail with a clear Linux requirement. Install/start/uninstall
operations require root. Status/log queries require Linux/systemd but do not
require root. Dry-run never writes system files.

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
4. Verify release artifact integrity with `SHA256SUMS`.
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

Package formats not yet implemented:

- `.deb`
- `.rpm`
- Homebrew formula
- Docker image
- Windows service package

Windows production service support is not currently published. Unsupported npm
platforms fail with a clear `unsupported platform` error.

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

Signature verification is not implemented yet and remains a follow-up hardening
task.

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
`.github/workflows/npm-release.yml`. Store an npm automation token in the
repository secret `NPM_TOKEN`, bump all package versions, then push a matching
tag such as `v0.1.2`.
