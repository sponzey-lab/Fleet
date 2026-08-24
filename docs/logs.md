# Sponzey Fleet Logs

Current log handling is a field diagnostic surface. It is not a log aggregation
or archival product.

## Agent Operational Log Upload

`sponzey agent start` uploads a product-safe operational log chunk by default.
The default interval is 30 seconds.

```bash
sponzey agent start
sponzey agent start --log-upload-interval-seconds 60
sponzey agent start --disable-log-upload
```

Current behavior:

- uploads agent operational status lines, not arbitrary system log files,
- defaults to enabled with a 30 second interval,
- can be disabled explicitly with `--disable-log-upload`,
- rejects a zero second upload interval,
- redacts secret-like values before sending,
- controller stores received chunks in `agent_log_chunks`.

The upload interval is independent from heartbeat, facts, metrics, and task
dispatch. Heartbeat is only the liveness signal for the persistent agent
session. Operational log chunks are sent on their own interval and do not wait
for command output or task completion.

Stored operational log chunks are available through the controller API:

```http
GET /api/agents/{agent_id}/logs?limit=50&before=<cursor>
Authorization: Bearer <admin-token>
```

The endpoint returns newest chunks first with the same cursor paging contract as
facts, metrics, and drift pages. `next_cursor` is passed back as `before` to
read older chunks. This stream is intentionally separate from
`GET /api/jobs/{job_id}/output`, which is the only API surface for command
stdout/stderr chunks.

## File Tail

```bash
sponzey logs --file /var/log/syslog
sponzey logs web-01 --file /var/log/syslog --follow --max-duration-seconds 30
```

Current behavior:

- reads the target file from the local process filesystem,
- emits the last 50 lines first,
- redacts secret-like values before display,
- truncates oversized lines,
- with `--follow`, polls the same file for appended lines,
- with `--max-duration-seconds`, exits the follow loop after the requested duration.

The optional `target` argument is accepted for operator context, but current file
tail does not yet open a remote file through an agent task.

## Journald Shortcut Skeleton

When no `--file` is provided and the target looks like a safe systemd unit name, the CLI renders the intended journald command:

```bash
sponzey logs nginx.service
```

This is a skeleton for the later systemd/journald adapter. It validates the service name and does not shell-execute untrusted input.

## Boundaries

- Product application logs do not include tailed log lines.
- Log stream output is redacted independently from application logging.
- Agent operational log chunks are persisted in `agent_log_chunks` and cleaned by the controller retention worker or explicit retention cleanup command.
- Raw file tail and journald stream artifacts are not persisted separately in MVP.
- Remote raw file or journald log streaming remains a later signed task/streaming protocol feature.
