# Sponzey Fleet

[한국어 설명서](README.ko.md)

Sponzey Fleet lets you manage several computers from one place. You install the
same `fleet` program on every machine, then choose what that machine should do:

- The **Controller** is the control center. It stores data, shows the Web Admin
  page, signs jobs, and receives connections from agents.
- An **Agent** runs on each managed computer. It connects to the Controller,
  reports inventory and metrics, and runs jobs signed by the Controller.

One Controller can manage many Agents. The Controller never needs to open an
incoming connection to an Agent; every Agent connects outward to the Controller.

> The npm package is named `@sponzey/fleet`, but the current command is
> `fleet`.

## What you need

For the easiest start, prepare:

- macOS or Linux
- Node.js and npm
- a terminal window
- a web browser

Check that Node.js and npm are installed:

```bash
node --version
npm --version
```

If either command is missing, install a current Node.js LTS release first.

## Install

Install Sponzey Fleet globally so the `fleet` command is available in every
terminal:

```bash
npm install -g @sponzey/fleet
fleet --version
```

If you install it only inside a project with `npm install @sponzey/fleet`, run it
as `npx fleet` instead.

If the terminal says `fleet: command not found`, inspect npm's global command
directory:

```bash
echo "$(npm prefix -g)/bin"
```

Add the printed directory to your shell `PATH`, then open a new terminal.

To build directly from this repository:

```bash
cargo build -p fleet-cli
./target/debug/fleet --version
```

When using the source build, replace `fleet` in the examples below with
`./target/debug/fleet`.

## Try the one-command demo

Before setting up real machines, run:

```bash
fleet demo
```

The demo creates temporary Controller and Agent data, runs a small job, and
prints a Web Admin URL. It removes the temporary data when it finishes.

## Understand the three important values

You will see these values during setup:

| Value | What it is | Where to use it |
| --- | --- | --- |
| Admin token | The password for operators | Paste it into Web Admin or use it with protected CLI/API commands |
| Enrollment token | A short-lived, usually one-time Agent registration code | Use it only with `fleet agent init` |
| Data directory | A folder containing keys, settings, and Controller or Agent data | Use the same directory every time you start that role |

The admin token and enrollment token are different. Do not use one in place of
the other.

## Beginner setup: Controller and Agent on one computer

This is the safest way to learn. You will use two terminal windows on the same
computer.

### Step 1: initialize the Controller

Open the first terminal and run:

```bash
mkdir -p fleet-controller
fleet controller init --data-dir ./fleet-controller
```

The command prints an `admin token`. Copy it into a password manager or another
safe temporary place. The Controller stores only its hash and does not show the
same raw token again.

Initialization is normally done only once.

### Step 2: start the Controller

In the same terminal, run:

```bash
fleet controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir ./fleet-controller \
  --external-url http://127.0.0.1:7700
```

Leave this terminal open. The Controller stops if you press `Ctrl+C` or close
the terminal.

`http://` is acceptable for this same-computer lesson only. Sponzey prints a
warning to remind you that HTTP is not encrypted.

### Step 3: open Web Admin

Open this address in your browser:

```text
http://127.0.0.1:7700/admin
```

Paste the admin token from Step 1. You should now see Web Admin, even though no
Agent is connected yet.

### Step 4: create an Agent enrollment token

Open a second terminal. Run this command while the Controller remains running:

```bash
fleet enroll-token create \
  --data-dir ./fleet-controller \
  --labels role=test,env=local
```

Copy the token printed by the command. This is the enrollment token, not the
admin token. You can also create enrollment tokens from Web Admin.

### Step 5: initialize the Agent

In the second terminal, replace `PASTE_ENROLLMENT_TOKEN_HERE` with the token from
Step 4, then run:

```bash
mkdir -p fleet-agent
fleet agent init \
  --data-dir ./fleet-agent \
  --url http://127.0.0.1:7700 \
  --token PASTE_ENROLLMENT_TOKEN_HERE \
  --name my-first-agent \
  --labels role=test,env=local
```

Agent initialization creates an Agent identity and pins the Controller identity.
It is normally done only once per Agent data directory.

### Step 6: start the Agent

Run:

```bash
fleet agent start --data-dir ./fleet-agent
```

Leave this terminal open too. Refresh Web Admin. `my-first-agent` should appear
in the Agent list.

To perform only one connection check and exit, use:

```bash
fleet agent start --data-dir ./fleet-agent --once
```

The normal Agent keeps reconnecting when the network or Controller is briefly
unavailable.

## Real setup: Controller and Agent on different computers

The order is the same, but the Agent must use the Controller computer's real IP
address or DNS name.

The examples below use `192.168.0.10` as the Controller address. Replace it with
your own value.

### Step 1: find the Controller computer's address

On Linux, try:

```bash
hostname -I
```

On macOS Wi-Fi, try:

```bash
ipconfig getifaddr en0
```

You may also find the address in your router or cloud server dashboard.

### Step 2: initialize and start the Controller

On the Controller computer:

```bash
mkdir -p fleet-controller
fleet controller init --data-dir ./fleet-controller
```

Save the printed admin token, then start the Controller:

```bash
fleet controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir ./fleet-controller \
  --external-url http://192.168.0.10:7700
```

Two addresses have different meanings here:

- `--host 0.0.0.0` means “listen on all network interfaces.” It is a bind value,
  not an address that an Agent can use.
- `--external-url http://192.168.0.10:7700` is the address Agents and operators
  actually use.

Never put `0.0.0.0` in `--external-url` or `agent init --url`.

Your firewall must allow the chosen port. If another computer cannot open
`http://192.168.0.10:7700/admin`, check the Controller process, IP address,
router/network rules, and firewall before continuing.

### Step 3: create an enrollment token on the Controller

On the Controller computer:

```bash
fleet enroll-token create \
  --data-dir ./fleet-controller \
  --labels role=web,env=test
```

Transfer the printed token securely to the Agent computer. Enrollment tokens
are short-lived and should not be posted in chat rooms, tickets, or source code.

### Step 4: install and initialize the Agent computer

On the Agent computer:

```bash
npm install -g @sponzey/fleet
mkdir -p fleet-agent
fleet agent init \
  --data-dir ./fleet-agent \
  --url http://192.168.0.10:7700 \
  --token PASTE_ENROLLMENT_TOKEN_HERE \
  --name web-01 \
  --labels role=web,env=test
```

Then start it:

```bash
fleet agent start --data-dir ./fleet-agent
```

Open `http://192.168.0.10:7700/admin` and confirm that `web-01` is online.

> `127.0.0.1` always means “this same computer.” An Agent on a second computer
> must not use `127.0.0.1` unless it connects through a local SSH tunnel.

## Use HTTPS for real or long-running environments

Plain HTTP does not encrypt admin tokens, enrollment traffic, jobs, or Agent
data. Use HTTP only for local learning, a private lab, or short-lived testing.
Use HTTPS for production, customer, shared, Internet-facing, or long-running
installations.

### Option A: built-in HTTPS

Prepare a certificate chain and private key on the Controller, then run:

```bash
fleet controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir /var/lib/fleet \
  --external-url https://fleet.example.com:7700 \
  --tls-cert /etc/fleet/tls/fullchain.pem \
  --tls-key /etc/fleet/tls/privkey.pem
```

The private key should be readable only by the account running the Controller.

Initialize Agents with the same HTTPS URL:

```bash
fleet agent init \
  --data-dir /var/lib/fleet \
  --url https://fleet.example.com:7700 \
  --token PASTE_ENROLLMENT_TOKEN_HERE \
  --name web-01 \
  --labels role=web,env=prod
```

If the server certificate uses a private or self-signed CA, add:

```text
--tls-ca-cert /path/to/ca.pem
```

### Option B: HTTPS reverse proxy

Nginx, Caddy, a cloud load balancer, or another reverse proxy can handle HTTPS.
In that case, keep Sponzey on Controller loopback:

```bash
fleet controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir /var/lib/fleet \
  --external-url https://fleet.example.com
```

Configure the proxy to forward HTTPS requests to `127.0.0.1:7700`, including
WebSocket connections.

## What you can do in Web Admin

After an Agent connects, Web Admin can:

- show Agent online/offline state and inventory
- display facts, metrics, drift history, and product-safe Agent logs
- create command and runbook jobs
- preview selector targets before creating a multi-Agent job
- show per-Agent assignment state and job output
- create and decide approval requests
- save and assign policies and schedule drift checks
- show remediation progress and audit events

Jobs are stored before they are sent. If an Agent is offline, its assignment
stays queued and can be delivered after the Agent reconnects.

## Policies and remediation in plain language

A **runbook** says, “perform these steps now.” A **policy** says, “this condition
should remain true over time.”

For example, a policy can say that nginx should be running on every Agent with
the label `role=web`.

The remediation flow is:

1. A signed drift check reports that the machine does not match the policy.
2. The Controller creates a remediation proposal.
3. An operator reviews and approves it.
4. The Controller creates and signs a runbook job.
5. The Agent runs the job and reports the result.
6. The Controller runs a verification check.
7. Fresh compliant evidence resolves the remediation and its original drift.

Remediation does not bypass approval. The old manual `running`, `result`, and
`verify` API/CLI commands are deprecated and return `409`; authenticated Agent
events and stored verification evidence are the source of truth.

See [Runbooks](docs/runbooks.md), [Policies](docs/policy.md), and the
[API contract](docs/api.md) for complete schemas and advanced examples.

## Optional operator CLI login

Web Admin is the easiest operator interface. If you prefer the CLI, save a
Controller URL and admin token in a local profile:

```bash
fleet login \
  --controller-url https://fleet.example.com \
  --admin-token PASTE_ADMIN_TOKEN_HERE
```

Then commands such as these use that profile:

```bash
fleet agents remote-list
fleet jobs list
fleet approvals list
fleet remediations list
fleet audit export --category security --limit 100
```

The profile contains an operator credential. Do not copy it to other users or
commit it to source control.

## Running continuously on Linux

On a Linux system using systemd, first initialize the Controller or Agent with
the same persistent data directory that the service will use. Preview the unit
before installing it:

```bash
fleet controller install-service \
  --data-dir /var/lib/fleet \
  --dry-run

fleet agent install-service \
  --data-dir /var/lib/fleet \
  --dry-run
```

Installing or removing a service requires Linux and root privileges:

```bash
sudo fleet agent install-service --data-dir /var/lib/fleet
sudo fleet agent start-service
sudo fleet agent status-service
sudo fleet agent logs-service
```

Controller service commands have the same shape. Verify the dry-run output and
your HTTPS/reverse-proxy settings before relying on a service in production.

## Back up the Controller

Back up before an upgrade, migration, or machine move. Stop the Controller first
so SQLite is not being written during the backup.

```bash
fleet controller backup \
  --data-dir ./fleet-controller \
  --output ./fleet-controller.backup.json
```

The backup contains Controller keys and operational data. Treat it as a secret.

Validate a backup without writing anything:

```bash
fleet controller restore \
  --data-dir ./restore-check \
  --input ./fleet-controller.backup.json \
  --dry-run
```

Restore into an empty data directory:

```bash
fleet controller restore \
  --data-dir ./fleet-controller-restored \
  --input ./fleet-controller.backup.json
```

## Common problems

### `fleet: command not found`

Open a new terminal after global installation. If it still fails, check
`$(npm prefix -g)/bin` and add that directory to `PATH`.

### `controller is not initialized`

Run `fleet controller init` once. Make sure `controller init` and
`controller start` use exactly the same `--data-dir`.

### `agent is not enrolled`

Run `fleet agent init` once. Make sure `agent init` and `agent start` use
exactly the same Agent `--data-dir`.

### The Agent cannot connect

Check these items in order:

1. Is the Controller terminal or service still running?
2. Is the Agent using the Controller IP/DNS name instead of its own
   `127.0.0.1`?
3. Is port `7700` open in the operating-system and cloud firewalls?
4. Does the URL start with `https://` when the Controller uses TLS?
5. Does a private CA require `--tls-ca-cert` during Agent initialization?

### The Agent does not appear in Web Admin

Check the Agent terminal for enrollment, identity, or connection errors. An
enrollment token is usually one-time use; create a new one instead of retrying a
consumed token.

### `WARNING: insecure HTTP controller URL enabled`

This is a warning, not a crash. It means traffic is not encrypted. Use HTTPS
outside local or short-lived testing.

### Web Admin shows `{"error":"not_found"}`

Open `/admin`, for example `http://127.0.0.1:7700/admin`.

### A job stays queued

The target Agent may be offline or waiting behind another assignment. Start the
Agent and inspect the Job and Audit views. Queued work is not silently treated
as completed.

## Security reminders

- Never commit admin tokens, enrollment tokens, private keys, or backup files.
- Use HTTPS for real environments.
- Keep Controller, Agent, and TLS private-key files readable only by the service
  account that needs them.
- Review target preview and approval details before running privileged jobs.
- Back up the Controller before upgrades or destructive maintenance.

More detail is available in [Security](docs/security.md),
[Storage](docs/storage.md), and the [feature matrix](docs/feature-matrix.md).

## API and development documentation

The Controller serves:

```text
/admin         Web Admin
/openapi.json  OpenAPI 3.1 JSON
/swagger-ui    Interactive API documentation
```

Do not enter real admin tokens in Swagger UI over plain HTTP.

For contributors, the main checks are:

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test --workspace @sponzey/fleet
npm test --workspace web-admin
npm run typecheck --workspace web-admin
npm run build --workspace web-admin
```

See the [release gate](docs/release-gate.md) for the complete release procedure.

## License

Sponzey Fleet is licensed under the GNU Affero General Public License version 3
only (`AGPL-3.0-only`). The license covers the Rust workspace, Web Admin, npm
wrapper, and distributed binaries unless a file explicitly says otherwise.
See [LICENSE](LICENSE) and [license notes](docs/license.md).
