# Sponzey Fleet

[한국어 문서](README.ko.md)

Sponzey Fleet is an agent-based server operations tool. It is distributed as one
`sponzey` binary. The role is selected by the command you run.

```text
sponzey controller ...
sponzey agent ...
sponzey enroll-token ...
sponzey run ...
sponzey demo
```

The core runtime is Rust. The npm package only installs the Rust binary.

## Simple Picture

| Part       | Where it runs                           | What it does                                                                                       |
| ---------- | --------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Controller | The machine operators open in a browser | Stores the database, serves Web Admin UI, creates enrollment tokens, receives agents, signs tasks. |
| Agent      | Each machine you want to manage         | Connects to the controller, sends health/facts/metrics, runs controller-signed tasks.              |

One controller can manage many agents.

Important terms:

| Term             | Meaning                                                                                                   |
| ---------------- | --------------------------------------------------------------------------------------------------------- |
| Data directory   | Folder where Sponzey stores keys, database, and local settings. Local examples use `.sponzey`.            |
| Admin token      | Printed by `sponzey controller init`. Use it only for the Web Admin UI and protected APIs.                |
| Enrollment token | Created by `sponzey enroll-token create`. Use it once when registering an agent.                          |
| Controller URL   | Address agents use to reach the controller. The setup flow is the same whether the URL is local or HTTPS. |

Facts and metrics mean different things:

| Data    | Meaning                                                                                           |
| ------- | ------------------------------------------------------------------------------------------------- |
| Facts   | Mostly stable inventory, such as OS, architecture, hostname, CPU core count, memory total/modules, disk devices, mount layout, disk capacity, and network interfaces. |
| Metrics | Time-series usage telemetry, such as CPU usage, memory usage, disk usage, process count, and service failure counts. |

## Install

```bash
npm install -g @sponzey/fleet
sponzey --help
```

If `sponzey` is not found after installation, your npm global bin directory is
not in `PATH`. The installer creates the npm global `sponzey` launcher when it
can, and also tries to create a PATH-visible launcher in a safe writable bin
directory such as `/usr/local/bin`. The installer does not silently edit shell
profile files. If your shell still cannot find `sponzey`, check the npm bin
directory with:

```bash
echo "$(npm prefix -g)/bin"
```

Then add that directory to your shell `PATH`, for example:

```bash
export PATH="$(npm prefix -g)/bin:$PATH"
```

From this source repository:

```bash
cargo build -p fleet-cli
./target/debug/sponzey --help
```

If you use the source build, replace `sponzey` below with `./target/debug/sponzey`.

### Install Paths

Use npm for the simplest developer and small-server install:

```bash
npm install -g @sponzey/fleet
```

Use standalone release archives when you do not want npm on the target host.
Release archives are named:

```text
sponzey-darwin-arm64.tar.gz
sponzey-darwin-x64.tar.gz
sponzey-linux-arm64.tar.gz
sponzey-linux-x64.tar.gz
```

Verify the archive checksum before installing:

```bash
./scripts/verify_standalone_artifacts.sh dist/release
```

The release workflow publishes `SHA256SUMS` with the archives. Signature
verification is not implemented yet; treat checksum verification plus release
provenance as the current integrity boundary.

For long-running Linux hosts, install the resolved binary as a systemd service:

```bash
sponzey controller install-service --data-dir /var/lib/sponzey-fleet --dry-run
sudo sponzey controller install-service --data-dir /var/lib/sponzey-fleet
sponzey controller status-service --dry-run
sponzey controller logs-service --dry-run
```

Agent services use the same shape with `sponzey agent install-service`.
Service units pass explicit CLI arguments and do not patch process environment
at runtime.

Upgrade is currently an external package/artifact operation. Inspect the policy
before replacing a binary:

```bash
sponzey upgrade --dry-run
```

Back up controller data before any upgrade that may touch controller storage.

## Fastest Demo

```bash
sponzey demo
```

This starts a temporary controller, enrolls a temporary agent, runs a sample job,
and prints the Web Admin URL.

## API Documentation

The Controller serves the operator UI at `/admin`. External REST API
documentation is available as OpenAPI 3.1 JSON and Swagger UI:

```text
GET /openapi.json
GET /swagger-ui
```

Protected API calls use the admin token printed by `sponzey controller init` as
a Bearer token. Do not use Swagger UI over HTTP except for local or short-lived
tests because tokens and request payloads are not encrypted. The detailed API
contract, public/internal endpoint boundary, pagination shape, and deprecation
policy are maintained in [docs/api.md](docs/api.md). The agent WebSocket
protocol is documented separately in [docs/protocol.md](docs/protocol.md).

The current bootstrap admin token maps to the `bootstrap-admin` actor with the
`owner` role. Minimal role and permission boundaries are documented in
[docs/security.md](docs/security.md).

Current implementation status is tracked in
[docs/feature-matrix.md](docs/feature-matrix.md). Release verification commands
and required smoke checks are tracked in [docs/release-gate.md](docs/release-gate.md).

## Transport Safety Warning

HTTP controller URLs are supported for setup checks, local development, lab
testing, and short-lived validation only. Treat HTTP as a test-only transport.

For any product, customer, production, shared, or long-running environment, you
must use HTTPS. If you choose to run Sponzey over HTTP, controller-agent traffic
is not encrypted. HTTP transport provides no confidentiality or integrity
guarantee and can expose tokens, commands, operational data, and traffic to
man-in-the-middle attacks.

## Pick Your Values First

The setup steps are always the same. The examples below use local values so you
can copy them first:

```text
DATA_DIR:        .sponzey
CONTROLLER_URL: http://127.0.0.1:7700
```

When you move to a real remote controller, change only the values:

- Use a production data directory such as `/var/lib/sponzey-fleet`.
- Use a controller URL such as `http://192.168.0.10:7700` or `https://fleet.example.com`.
- Use `http://` only for tests. Use `https://` for product or production use.
- If the controller URL starts with `http://`, Sponzey prints a warning every time because controller-agent traffic is not encrypted.
- If you want HTTPS, finish [HTTPS Preparation](#https-preparation) first.

## One Setup Flow

Use this same order for local testing, SSH tunnel development, test-only HTTP
remote use, and HTTPS remote use. The commands here are the local copy-and-paste
version. For a real remote controller, replace only the data directory,
controller URL, name, labels, and token.

### 1. Initialize The Controller

Run once on the controller machine:

```bash
sponzey controller init --data-dir .sponzey
```

Copy the `admin token` printed by this command. You will paste it into the Web
Admin UI.

### 2. Start The Controller

```bash
sponzey controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir .sponzey \
  --external-url http://127.0.0.1:7700
```

Keep the controller terminal open.

### 3. Open Web Admin

Open the controller URL with `/admin` at the end.

```text
http://127.0.0.1:7700/admin
```

Paste the admin token from step 1.

The Web Admin surface shows agent inventory, selected agent details, facts,
disk and mount inventory, metrics charts with a range selector, drift latest
and history, agent operational logs, job output, per-target assignment state,
pending approvals, enrollment tokens, policy assignment, runbook job creation,
audit events, and an HTTP transport warning banner when opened over HTTP.

### 4. Create An Enrollment Token

Run this on the controller machine:

```bash
TOKEN=$(sponzey enroll-token create \
  --data-dir .sponzey \
  --labels role=web,env=dev)
```

This token is for the agent. It is not the admin token.

You can also print a ready-to-run agent command:

```bash
sponzey enroll-token create \
  --data-dir .sponzey \
  --labels role=web,env=dev \
  --controller-url http://127.0.0.1:7700 \
  --name web-01 \
  --print-init-command
```

### 5. Initialize The Agent

Run once on the agent machine:

```bash
sponzey agent init \
  --data-dir .sponzey \
  --url http://127.0.0.1:7700 \
  --token "$TOKEN" \
  --name web-01 \
  --labels role=web,env=dev
```

### 6. Start The Agent

For a one-time check:

```bash
sponzey agent start \
  --data-dir .sponzey \
  --once
```

For a continuous local agent:

```bash
sponzey agent start \
  --data-dir .sponzey
```

Refresh Web Admin. The agent should appear in the agent list.

`agent start` is meant to stay alive. If the controller is temporarily down or
the network is unavailable, it keeps retrying by default. Use `--once` for a
single smoke check, or `--max-reconnect-attempts <N>` when you explicitly want
the process to exit after repeated connection failures.

By default, the agent also uploads product-safe operational log chunks every
30 seconds. These are agent status events, not raw system log files. Change the
interval with `--log-upload-interval-seconds <SECONDS>`, or disable this upload
with `--disable-log-upload`.

Heartbeat, facts, metrics, and operational logs have separate intervals.
Heartbeat is only the liveness tick and does not control task dispatch. Static
inventory facts default to every 300 seconds with `--facts-interval-seconds`.
Usage metrics default to every 30 seconds with `--metrics-interval-seconds`.
Task assignments are pushed on the persistent session independently from those
telemetry intervals.

## How Run Works

The controller does not open a connection to an agent. Each agent opens one
outbound persistent WebSocket session to the controller after enrollment.

When you run a command from Web Admin or `sponzey run`, the controller first
stores the job and its signed task assignment. If the target agent is currently
connected, the controller pushes the task immediately over that existing
session. The run path does not wait for the next heartbeat. Heartbeat is only a
liveness signal.

If the target agent is offline, the job stays queued. When the agent reconnects
and authenticates again, the controller drains pending assignments for that
agent and pushes the next task over the renewed session.

The agent returns `output_chunk` messages and one `task_result` over the same
session. Web Admin polls the job detail and output APIs as a fallback display
path, so the UI can show queued, delivered, running, completed, and no-output
states without embedding raw command output in product logs.

Revoking an agent key disables the agent, closes any active session with the
`agent_revoked` reason, and blocks additional task delivery. Revoke is not the
job stop button. To stop a specific job, call `POST /api/jobs/{job_id}/cancel`
or use the equivalent UI/CLI surface when available.

Cancel records the job and assignment as `canceled`. If the agent session is
active and the task was already dispatched, the controller sends `task_cancel`
over the existing WebSocket session. The agent kills the current command
process when the task id matches and reports `task_result.status = "canceled"`.
Command timeout is separate: timeout reports `task_result.status = "timed_out"`
and the controller stores the job as `expired`.

## Target Preview and Snapshots

Before creating a job, automation or Web Admin can call `POST
/api/selectors/preview` with either a string selector or `matchLabels`:

```json
{ "matchLabels": { "role": "web", "env": "prod" } }
```

Supported string selectors are `agent:<name-or-id>`, `label:key=value`, and
`key=value,key2=value2`. Disabled or revoked agents are shown in preview but
excluded from dispatch. Offline agents can be selected; their assignments stay
queued until they reconnect.

When a job is created, the controller stores the selector source and a target
snapshot. Later label or status changes do not change the job's original target
set.

For multi-agent jobs, create the job after checking the preview result. The
controller creates one assignment per target in that snapshot. The optional job
`strategy` controls fanout:

```json
{
  "strategy": {
    "concurrency": 2,
    "maxFailures": 1
  }
}
```

`concurrency` defaults to `1`, which means sequential dispatch. `maxFailures`
is optional; when the threshold is reached, remaining queued assignments are
canceled instead of being dispatched. Job detail responses include the saved
strategy, per-target `task_id`, `assignment_status`, and `last_error` fields,
plus an `assignment_summary` count object so Web Admin and automation can
distinguish connectivity from execution state.

## Risky Jobs And Approval

Sponzey separates creating a risky job from dispatching it to an agent.

Safe single-agent probes such as `uptime` can be queued immediately. Shell
commands, `sudo`, `su`, reboot/shutdown actions, user/group changes,
package/service/file mutations, unknown commands, and broad multi-agent targets
create an approval request instead. The job stays in `pending_approval` and is
not dispatched until the approval is approved.

`confirmed_high_risk` and `--confirm-risk` are compatibility acknowledgements.
They do not replace approval. An approval records the approver, reason, status,
expiry, and audit events.

The approver is derived from the authenticated admin token. Approval request
bodies can include a reason; UI-provided actor fields are not trusted for audit
or authorization.

The approval API is available now:

```text
GET  /api/approvals?status=pending
POST /api/approvals/{approval_id}/approve
POST /api/approvals/{approval_id}/reject
POST /api/approvals/expire
```

The Web Admin approval queue uses the same API. Approve/reject actions send only
the decision reason; the controller derives the approver from the authenticated
admin token and then refreshes approval, job, and audit views.

## HTTPS Preparation

You need this section for product, customer, production, shared, or long-running
use. HTTP works without this section, but HTTP is test-only and Sponzey will
keep printing an insecure HTTP warning.

There are two common ways to provide HTTPS. This section is preparation, not a
second setup flow.

After HTTPS is ready, go back to [One Setup Flow](#one-setup-flow) and replace
the local values:

- `http://127.0.0.1:7700` becomes your HTTPS controller URL.
- `.sponzey` becomes your production data directory if needed.
- `agent start` uses the production data directory.

If your HTTPS certificate is private or self-signed, add this to `agent init`:

```bash
--tls-ca-cert /path/to/ca.pem
```

### Built-In HTTPS

Prepare these files on the controller machine:

```text
/etc/sponzey/tls/fullchain.pem
/etc/sponzey/tls/privkey.pem
```

The private key must not be readable by other users.

```bash
sudo chmod 600 /etc/sponzey/tls/privkey.pem
```

Start the controller:

```bash
sponzey controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir /var/lib/sponzey-fleet \
  --external-url https://fleet.example.com:7700 \
  --tls-cert /etc/sponzey/tls/fullchain.pem \
  --tls-key /etc/sponzey/tls/privkey.pem
```

### Reverse Proxy HTTPS

Use this when Nginx, Caddy, a load balancer, or another proxy handles HTTPS.
Sponzey can stay on loopback:

```bash
sponzey controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir /var/lib/sponzey-fleet \
  --external-url https://fleet.example.com
```

Your proxy should forward HTTPS traffic to `127.0.0.1:7700`.

## SSH Tunnel Development

SSH tunnel development uses the same setup flow. The only difference is that the
agent reaches the controller through a local tunnel URL.

On the agent machine, keep this running:

```bash
ssh -N -L 7700:127.0.0.1:7700 <user>@<controller-host>
```

Then use this URL on the agent machine:

```text
http://127.0.0.1:7700
```

If you use the controller machine's LAN IP with plain `http://`, Sponzey allows
it but prints an insecure HTTP warning.

## Local Scripts

The scripts are shortcuts around the same single binary.

```bash
./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir .sponzey --external-url http://127.0.0.1:7700
./scripts/run_agent.sh --data-dir .sponzey
```

Important:

- `run_controller.sh` wraps `sponzey controller start`.
- `run_agent.sh` wraps `sponzey agent start`.
- The scripts do not run `controller init`, `enroll-token create`, or `agent init`.
- Do not run `scripts/run_agent.sh controller ...`; that script is agent-only.

## Remove An Agent

Stop the agent first.

If you installed a systemd service:

```bash
sponzey agent uninstall-service --dry-run
sudo sponzey agent uninstall-service
```

Then remove the local agent directory:

```bash
rm -rf .sponzey/agent
```

For a production data directory:

```bash
sudo rm -rf /var/lib/sponzey-fleet/agent
```

Controller inventory and audit records are kept. To use the same host again,
create a new enrollment token and run `sponzey agent init` again.

## Back Up And Restore Controller Data

Back up the controller before deleting a data directory, moving to another
machine, or performing risky maintenance. Stop the controller first so the
SQLite database is not being written while the backup is created.

```bash
sponzey controller backup \
  --data-dir .sponzey \
  --output ./sponzey-controller.backup.json
```

The backup archive contains sensitive controller state, including the controller
identity keys and SQLite data. Store it like a secret.

Validate a restore without writing files:

```bash
sponzey controller restore \
  --data-dir ./restore-check \
  --input ./sponzey-controller.backup.json \
  --dry-run
```

Restore into an empty data directory:

```bash
sponzey controller restore \
  --data-dir .sponzey-restored \
  --input ./sponzey-controller.backup.json
```

Restore refuses to overwrite an existing controller directory. Use `--force`
only after you have confirmed the target data directory can be replaced.

To reset everything, remove the whole data directory:

```bash
rm -rf .sponzey
```

Deleting the data directory is a reset. Backup/restore preserves controller
identity, inventory, jobs, audit events, telemetry, and enrollment records.

## Common Problems

### `controller is not initialized`

Run `controller init` once with the same data directory.

### `unable to open database file`

The controller data directory was probably not initialized. Run
`sponzey controller init --data-dir ...` first.

### `agent is not enrolled`

Run `sponzey agent init ...` before `sponzey agent start ...`.

### A running job stays running after the agent disconnects

This is expected. The controller does not mark a job as failed just because the
WebSocket dropped. A final `task_result`, cancel, timeout, or expiry policy
decides the terminal state. Use job output and audit entries to confirm what
happened.

### Cancel, failed, and expired look different

`canceled` means an operator cancel was recorded. `failed` means the agent
reported a non-zero or failed result. `expired` means timeout or assignment
expiry won. These states are intentionally separate.

### `WARNING: insecure HTTP controller URL enabled`

This is not a crash. It means your controller URL starts with `http://`, so
controller-agent traffic is not encrypted. HTTP is test-only. Product,
customer, production, shared, or long-running environments must use HTTPS.
HTTP transport provides no confidentiality or integrity guarantee.

### Web Admin shows `{"error":"not_found"}`

Open `/admin`, not an API path.

### Which token goes where?

- Web Admin UI: use the admin token from `sponzey controller init`.
- Agent init: use the enrollment token from `sponzey enroll-token create`.

## Development Checks

The full release gate is documented in [docs/release-gate.md](docs/release-gate.md).

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test --workspace @sponzey/fleet
npm test --workspace web-admin
npm run typecheck --workspace web-admin
npm run build --workspace web-admin
./scripts/smoke_mvp.sh
./scripts/smoke_immediate_dispatch.sh
./scripts/smoke_remote_tls_loopback.sh
```
