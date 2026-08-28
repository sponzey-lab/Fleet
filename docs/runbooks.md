# Sponzey Fleet Runbooks

Runbooks are a small Sponzey-specific YAML DSL for idempotent server operation
steps. They are not Ansible playbooks and they do not aim for Ansible syntax
compatibility.

The purpose of the runbook contract is to make execution results predictable:
operators and API clients must be able to distinguish success, changed, skipped,
failed, rejected, canceled, diff, duration, and per-step output without reading
raw product logs.

## Validation

```bash
fleet apply examples/runbooks/nginx-basic.yml
```

Current `apply` behavior remains validation-only:

- parses the runbook,
- validates required fields,
- rejects unsupported top-level fields,
- rejects unsupported task kinds and task fields,
- rejects unsafe `file.copy.dest` paths,
- lowers supported tasks into a primitive execution plan,
- does not execute package, service, or file changes directly.

Execution remains behind controller-signed task envelopes, approval, and audit.
The controller exposes that execution path through `POST /api/jobs/runbook`.

## Canonical Schema

Canonical v1alpha1 runbooks use top-level operational fields:

```yaml
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: nginx-basic
description: Install nginx and make sure the service is running.
matchLabels:
  role: web
  env: prod
strategy:
  concurrency: 1
  maxFailures: 1
checkMode: false
dryRun: false
steps:
  - id: nginx-package
    package:
      name: nginx
      state: present
  - id: nginx-service
    service:
      name: nginx
      state: started
      enabled: true
```

Required fields:

- `apiVersion`: must be `fleet.sponzey.dev/v1alpha1`.
- `kind`: must be `Runbook`.
- `name`: stable runbook name.
- `selector` or `matchLabels`: target selector inside the document.
- `steps`: non-empty step list.

Optional fields:

- `description`: human-readable purpose.
- `strategy.concurrency`: positive integer, defaults to `1`.
- `strategy.maxFailures`: positive integer, optional.
- `checkMode`: boolean, defaults to `false`.
- `dryRun`: boolean, defaults to `false`.

Selector rules:

- Use `selector` for string selectors such as `agent:web-01`, `label:role=web`,
  or `role=web,env=prod`.
- Use `matchLabels` for structured label selectors.
- Do not use `selector` and `matchLabels` together in the same runbook.
- A runbook job request may still provide explicit `target_agent_ids`,
  request-level `selector`, or request-level `matchLabels`. Those request
  fields take precedence. If all request target fields are empty, the
  controller resolves targets from the runbook document selector.

## Legacy Compatibility

The previous Kubernetes-like shape remains accepted as a compatibility fixture:

```yaml
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
metadata:
  name: nginx-basic
spec:
  targets:
    selector: role=web
  tasks:
    - id: nginx-package
      package:
        name: nginx
        state: present
```

New documents should use the canonical top-level shape. The legacy shape exists
so existing test runbooks and early deployments can still be parsed.

## Unknown Field Policy

The parser is intentionally strict.

- Unknown top-level fields are rejected.
- Unknown `spec` fields in the legacy shape are rejected.
- Unknown primitive fields are rejected.
- Unsupported YAML constructs are rejected.
- Tabs are rejected with a user-facing YAML error.

This keeps the DSL small enough to audit and avoids silently ignoring dangerous
operator intent.

## Strategy

Runbook `strategy` describes desired fanout behavior. Controller job request
strategy uses the same meaning:

- `concurrency`: how many target assignments may be dispatched at once.
- `maxFailures`: when this many target assignments fail, remaining queued
  assignments are canceled.

Request-level strategy is the controller dispatch strategy. Runbook document
strategy is the DSL-level default and validation contract. Current controller
requests may pass strategy explicitly; future catalog execution should default
to the runbook document strategy when the request omits one.

## Check Mode And Dry Run

`checkMode` and `dryRun` are intentionally different.

| Mode | Meaning |
| --- | --- |
| `dryRun` | Planning mode. The agent does not execute any primitive. Every step is returned as `skipped`. |
| `checkMode` | Inspection mode. Low-risk check steps can run, but mutation steps such as package install, service start, and file copy are skipped. |

`dryRun` is stronger than `checkMode`: it avoids every local side effect.
`checkMode` may still execute read-only commands such as package-present checks
or service status checks.

Approval is evaluated at the runbook job level. A runbook execution task is
high-risk as a whole because a single document can contain multiple mutation
steps. Step-level high-risk checks still exist on the agent so an unconfirmed or
incorrectly signed high-risk step cannot be executed by accident.

## Supported Primitives

Supported task declarations are intentionally split into safe read-only checks,
controlled idempotent mutations, and deferred dangerous primitives.

Safe read-only declarations:

- `port.check` with optional `host` and required `port`
- `process.check` with `name`
- `facts.collect` with optional `scope: local`
- `metrics.snapshot` with optional `scope: local`

Controlled mutation declarations:

- `package` with `name` and `state: present`
- `service` with `name`, `state: started|restarted`, optional `enabled: true|false`
- `file.copy` with absolute safe `dest`, inline `content`, optional `mode`
- `file.template` with absolute safe `dest`, inline `content`, optional `mode`,
  optional comma-separated `variables`, optional comma-separated `secretRefs`,
  and optional `checksum: sha256`

Deferred dangerous declarations:

- `shell`
- `reboot`
- `user`
- `group`
- `cron`
- unbounded `logs.tail`

Those dangerous declarations remain unsupported until approval, capability, raw
output retention, and redaction policies are implemented for them.

### Idempotent Mutation Semantics

`package` and `service` are compound primitives. The runner checks current state
before it executes a high-risk mutation command.

- `package state: present` first runs a package-present check. If the package is
  already installed, the result is `success` with `changed=false`; install is not
  invoked. If the package is missing and check mode is enabled, the result is
  `skipped` with `changed=true`. If the package is missing and the runbook has
  confirmed high-risk execution, the install command runs and a successful
  install returns `changed`.
- `service state: started` first checks `systemctl is-active`. If the service is
  already active and no enable action is requested, the result is `success` with
  `changed=false`; start is not invoked. If service mutation is needed in check
  mode, the result is `skipped` with `changed=true`. `state: restarted` is always
  treated as a mutation because restart intentionally changes runtime state.
- `file.copy` compares content before writing. Unchanged content returns
  `success` with `changed=false` and a SHA-256 diff where `before` and `after`
  match. Changed content writes through a same-directory temporary file and
  returns `changed` with before/after SHA-256 checksums.

The file-copy primitive is intentionally narrow:

- destination must be absolute and must not traverse through `..`,
- parent directory must already exist,
- content is written through a same-directory temporary file followed by rename,
- unchanged content returns `changed=false`,
- before/after SHA-256 checksums are returned in the diff,
- owner/group management is out of scope and must be modeled as a later explicit primitive.

The `file.template` primitive uses the same file write path after rendering:

- template syntax is limited to `{{ variable_name }}` replacement,
- variables come only from the runbook document `variables` field,
- `variables` uses comma-separated `name=value` pairs with ASCII identifier
  names,
- `secretRefs` uses comma-separated `name=secret://scope/name` pairs and stores
  only a validated `SecretRef` marker in the parsed runbook,
- `secretRefs` reject empty values, unsupported schemes, traversal segments,
  query strings, whitespace, and inline `token=`/`secret=` material,
- templates that reference a `secretRefs` variable fail unless an explicit
  SecretProvider-compatible resolver is injected into the runner planning path,
- secret provider mode is a startup-only setting. The default is disabled, and
  the current `static-test` settings mode is limited to JSON fixture paths for
  contract tests/local development; runbooks, API requests, and Web Admin UI do
  not select or modify the provider,
- controller bootstrap constructs the provider from typed settings only. A
  disabled provider denies every secret reference, and `static-test` requires an
  explicit fixture source supplied by local/test code rather than environment or
  request state,
- agent runbook execution passes the selected provider as an explicit resolver
  closure to the runner. The runner does not discover providers itself,
- `fleet apply` validates runbook structure and primitive planning only. It
  does not resolve `secretRefs`, read provider configuration, or prove that a
  secret-backed template can execute in a running agent context,
- unsupported Mustache control expressions such as sections, partials, comments,
  unescaped variables, external includes, and loops are rejected,
- rendered content is passed to the existing `file.copy` atomic write/checksum
  path,
- rendered artifact metadata is reported through the agent `task_result`
  protocol and stored by the controller,
- the reported artifact SHA-256 can be used as a policy `file.sha256` expected
  value for later drift checks,
- SecretProvider-backed rendering writes the resolved content to the destination
  file only; artifact metadata is still reported, but artifact `content_bytes`
  are omitted for secret-backed templates so raw rendered secrets are not sent
  to controller artifact body storage by default.

Example:

```yaml
steps:
  - id: nginx-template
    file.template:
      dest: /etc/nginx/conf.d/sponzey.conf
      content: server { listen {{ port }}; server_name {{ host }}; }
      mode: "0644"
      variables: port=8080,host=example.test
      checksum: sha256
```

### Check Primitives

`port.check` and `process.check` never report local state mutation. Their
`changed` field is always `false`.

Check failure is not a runner failure:

- A reachable port or running process returns `status=success`, `changed=false`.
- A closed port or missing process returns `status=failed`, `changed=false`.
- A malformed primitive or an execution engine error still returns a runner error
  and does not masquerade as a normal failed check.

Example:

```yaml
steps:
  - id: http-listener
    port.check:
      host: 127.0.0.1
      port: 8080
  - id: nginx-process
    process.check:
      name: nginx
```

### Snapshot Primitives

`facts.collect` and `metrics.snapshot` collect an immediate local snapshot for
the runbook result only. They do not write into the controller's periodic
facts/metrics storage tables and do not replace the agent's persistent session
collector.

The snapshot payload is returned as step `stdout` with `source=runbook` and
`system_time_ms`.

Example:

```yaml
steps:
  - id: facts-now
    facts.collect:
      scope: local
  - id: metrics-now
    metrics.snapshot:
      scope: local
```

`logs.tail` is deliberately not enabled as a runbook primitive yet. Raw logs need
bounded scope, redaction, output size limits, and retention rules before they can
be safely exposed through remote execution.

## Primitive Result Schema

Every primitive step result follows this common shape:

```json
{
  "id": "nginx-config:copy",
  "status": "changed",
  "changed": true,
  "message": "file copied",
  "diff": {
    "format": "sha256",
    "before": "old-checksum-or-null",
    "after": "new-checksum"
  },
  "started_at_ms": 1710000000000,
  "completed_at_ms": 1710000000125,
  "duration_ms": 125,
  "exit_code": null,
  "stdout": "",
  "stderr": "",
  "audit_metadata": "primitive=file.copy,destination=/etc/nginx/conf.d/sponzey.conf,changed=true,bytes=42"
}
```

Status values:

- `success`: step completed and did not report a change.
- `changed`: step completed and changed local state.
- `skipped`: step did not execute because of dry-run/check mode or future policy.
- `failed`: step executed and failed.
- `rejected`: step was rejected before execution.
- `canceled`: step was canceled.

`changed` remains an explicit boolean-like field because status alone is not
enough for all primitives. Some read-only checks can succeed while `changed` is
unknown, so `changed` may be `true`, `false`, or `null`.

`stdout` and `stderr` are job output data. They must not be written into product
application logs. Product logs should record only high-level status and audit
metadata.

Aggregate result rule:

- Any `canceled` step makes the aggregate `canceled`.
- Otherwise any `rejected` step makes the aggregate `rejected`.
- Otherwise any `failed` step makes the aggregate `failed`.
- Otherwise any `changed` step makes the aggregate `changed`.
- Otherwise all-skipped results aggregate to `skipped`.
- Otherwise the aggregate is `success`.

### Result Examples

Package already present:

```json
{
  "id": "nginx-package:package",
  "status": "success",
  "changed": false,
  "message": "package nginx is already present",
  "diff": null,
  "exit_code": 0,
  "stdout": "",
  "stderr": "",
  "audit_metadata": "primitive=package,name=nginx,state=present,changed=false"
}
```

Port check failed because the port is closed:

```json
{
  "id": "http-listener:port.check",
  "status": "failed",
  "changed": false,
  "message": "port 8080 on 127.0.0.1 is not reachable",
  "diff": null,
  "exit_code": null,
  "stdout": "connection failed",
  "stderr": "",
  "audit_metadata": "primitive=port.check,host=127.0.0.1,port=8080"
}
```

Runbook facts snapshot:

```json
{
  "id": "facts-now:facts.collect",
  "status": "success",
  "changed": false,
  "message": "facts snapshot collected",
  "diff": null,
  "exit_code": null,
  "stdout": "{\"kind\":\"facts\",\"source\":\"runbook\",\"system_time_ms\":1710000000000}",
  "stderr": "",
  "audit_metadata": "primitive=facts.collect"
}
```

## Signed Runbook Dispatch

The controller accepts a runbook job request at `POST /api/jobs/runbook`.

Minimal request shape:

```json
{
  "job_id": "job-nginx-runbook-1",
  "target_agent_ids": [],
  "runbook_document": "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nname: nginx-basic\nmatchLabels:\n  role: web\nsteps:\n  - id: nginx-package\n    package:\n      name: nginx\n      state: present\n",
  "timeout_seconds": 180,
  "confirmed_high_risk": true,
  "expires_in_seconds": 300
}
```

Rules:

- invalid runbooks are rejected before task assignment,
- request target fields override runbook document target fields,
- when request target fields are empty, runbook `selector` or `matchLabels`
  resolves the target snapshot,
- runbook jobs are high-risk and require approval before dispatch,
- `confirmed_high_risk` is compatibility acknowledgement and does not replace approval,
- disabled or revoked agents are excluded from dispatch targets,
- job output stores step stdout/stderr separately from product logs.

## Manual Linux Nginx Smoke

The repository includes an ignored runner integration test and a signed-dispatch
wrapper script for the destructive Linux check that cannot run in the default
macOS or CI path.

Requirements:

- Linux host
- root privileges
- systemd
- `apt-get`, `dnf`, `yum`, or `apk`

Run:

```bash
sudo ./scripts/manual_linux_nginx_runbook_smoke.sh
# or run it through the release gate
sudo ./scripts/release_readiness_gate.sh --include-manual
```

The script starts a local controller, enrolls a local agent, creates a signed
runbook job through `POST /api/jobs/runbook`, runs the agent once, installs nginx
when missing, enables/starts `nginx.service`, and verifies that the service is
active.
