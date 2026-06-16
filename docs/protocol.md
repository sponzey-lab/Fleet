# Sponzey Fleet Protocol

Sponzey Fleet agent-controller protocol은 JSON wire message를 사용한다. Rust domain object는 wire schema에 직접 노출하지 않고, `fleet-protocol`의 `WireMessage`와 `WirePayload`가 직렬화 경계가 된다.

## Envelope

모든 message는 공통 envelope를 가진다.

```json
{
  "protocol_version": 1,
  "message_id": "msg-1",
  "correlation_id": "corr-1",
  "agent_id": "agent-1",
  "timestamp_ms": 1,
  "payload": {
    "type": "heartbeat",
    "payload": {
      "agent_id": "agent-1",
      "status": "online"
    }
  }
}
```

필수 규칙:

- `protocol_version`은 현재 `1`이다.
- version mismatch는 reject한다.
- unknown message type은 reject한다.
- malformed JSON은 reject한다.
- `message_id`는 message 단위 식별자다.
- `correlation_id`는 request-response 또는 task-output 흐름을 묶는다.
- `agent_id`는 enrollment 이전 message에서는 없을 수 있다.
- `timestamp_ms`는 message를 만든 쪽의 시스템 시각이다. Agent가 보내는 facts, metrics, drift report message의 `timestamp_ms`는 controller 저장 시각과 API 응답의 `agent_system_time_ms` 기준이 된다.

## Auth/Session Payloads

인증과 session 유지용 payload:

- `enroll_request`
- `enroll_response`
- `agent_hello`
- `auth_challenge`
- `auth_response`
- `auth_accepted`
- `heartbeat`

예:

```json
{
  "type": "auth_challenge",
  "payload": {
    "nonce": "nonce-1"
  }
}
```

## WebSocket Gateway

MVP controller는 agent outbound 연결을 다음 endpoint에서 받는다.

```text
ws://127.0.0.1:7700/api/agents/ws
```

현재 handshake 흐름:

1. Agent sends `agent_hello` with `agent_id` and `fingerprint`.
2. Controller loads the enrolled agent public key and verifies the fingerprint.
3. Controller sends `auth_challenge` with nonce.
4. Agent signs the nonce with its local Ed25519 private key and sends `auth_response`.
5. Controller sends `auth_accepted`.
6. Controller registers the authenticated persistent session.
7. Controller drains at most one pending assignment for the agent immediately after session registration.
8. Agent sends periodic `heartbeat` liveness ticks on the same session.
   Facts, metrics, and operational log chunks use separate agent-side intervals.
9. New command, drift-check, or runbook jobs are stored first, then dispatched immediately to active sessions.
10. Agent verifies the signed envelope and executes the command, drift check, or runbook task.
11. Agent sends `output_chunk` messages and one `task_result`.

### Current WebSocket lifecycle

현재 lifecycle은 persistent outbound session이다.

- Agent가 Controller로 outbound WebSocket을 연다.
- 인증이 끝난 뒤 Agent가 heartbeat를 보낸다.
- Controller는 인증된 session을 runtime registry에 등록한다.
- session 등록 직후 해당 agent의 pending assignment를 최대 1개 drain한다.
- `POST /api/jobs/command`, `POST /api/jobs/runbook`, `POST /api/jobs/drift-check`는 job과 assignment를 DB에 저장한 뒤 active session으로 즉시 dispatch를 시도한다.
- connected session이 없거나 outbound queue가 가득 차면 assignment는 DB queued 상태로 남고 dispatch failure가 audit된다.
- queued assignment가 없더라도 Controller는 연결을 유지하고 heartbeat/facts/metrics/log/output/result payload를 같은 session에서 계속 처리한다.
- Controller는 Agent로 직접 접속하지 않는다. 현재 구조와 목표 구조 모두 Agent outbound 연결을 기준으로 한다.

Agent enrollment generates an Ed25519 key pair locally. The private key is stored in `agent_private.key`; the controller stores the public key and fingerprint. On Unix, `agent.conf` and `agent_private.key` must not be readable, writable, or executable by group/other.

`sponzey agent start` runs as a persistent session loop by default. `--heartbeat-interval-seconds` controls liveness ticks only, not the connection cycle, facts upload, metrics upload, log upload, or task dispatch. Facts default to a lower-frequency static inventory interval controlled by `--facts-interval-seconds` (300 seconds). Metrics default to a chart-friendly interval controlled by `--metrics-interval-seconds` (30 seconds). Controller connection failures are retried indefinitely unless `--once` or `--max-reconnect-attempts` is set. For smoke tests and one-shot checks, pass `--once`.

Controller session implementation note:

- 인증이 끝난 Controller WebSocket handler는 read loop와 write loop를 분리한다.
- session당 WebSocket writer는 하나의 writer loop만 소유한다.
- Controller 내부 producer는 bounded outbound channel로 `task_assignment` 같은 `WireMessage`를 전달한다.
- outbound queue overflow는 write queue overflow로 구분되며, dispatch 계층에서는 DB queued assignment를 source of truth로 유지해야 한다.
- read loop가 처리하는 heartbeat, facts, metrics, log, output, task result, drift, security event는 authenticated session agent id와 payload agent id를 비교한다.
- agent id mismatch payload는 저장하지 않고 security audit 대상으로 처리한다.
- DB write failure는 message만 조용히 무시하지 않고 `store_error` close reason으로 session cleanup 대상이 된다.
- raw command output은 일반 product log로 흘리지 않고 job output storage에만 저장한다.
- output chunk는 bounded runner output limit과 chunk sequence를 유지한다.
- 같은 `(job_id, agent_id, stream, sequence)` output chunk가 같은 body로 다시 오면 idempotent duplicate로 보고 무시한다.
- 같은 key의 output chunk가 다른 body로 오면 raw body를 audit/log에 남기지 않고 `websocket_output_chunk_conflict` security audit를 남긴 뒤 protocol error로 session cleanup 대상이 된다.
- controller connection drop만으로 `running` job을 즉시 `failed`로 바꾸지 않는다. task result가 없으면 기존 expiry/reconciler 정책이 최종 상태를 결정한다.
- Agent key revoke 성공 직후 controller는 active session이 있으면 `agent_revoked` close reason으로 writer loop에 close를 enqueue하고 registry에서 제거한다.
- Revoke는 추가 task 수신 차단과 session 종료를 보장한다. 특정 job을 중단하려면 revoke가 아니라 `POST /api/jobs/{job_id}/cancel`을 사용한다.
- Cancel API는 queued assignment를 DB에서 terminal `canceled`로 바꾸고, 이미 `dispatched`, `accepted`, `started` 상태인 active session에는 `task_cancel`을 보낸다.
- Agent는 `task_cancel`이 현재 실행 중인 task id와 일치하면 cancel flag를 설정하고, command runner는 child process를 kill한 뒤 `task_result.status = "canceled"`를 돌려보낸다.
- session lifecycle audit action은 `agent_session_started`, `agent_session_ended`, `agent_session_replaced`, `agent_session_revoked_closed`, `agent_session_auth_failed`를 사용한다.

Close reason policy:

- `normal_shutdown`: agent 또는 controller session loop가 정상 종료됐다.
- `handler_ended`: request handler가 끝나면서 registry guard가 정리됐다.
- `replaced_by_new_session`: 같은 agent id의 새 authenticated session이 이전 session을 대체했다.
- `agent_revoked`: agent key revoke로 active session이 닫혔다.
- `auth_failed`: authentication이 실패했다.
- `protocol_error`: malformed payload, invalid sequence, duplicate output body mismatch 같은 protocol 위반이다.
- `store_error`: controller가 session payload를 저장하지 못했다.
- `write_failure` / `write_queue_overflow`: WebSocket writer 또는 bounded outbound queue 문제다.

Security notes:

- HTTP/WebSocket controller URLs are test-only and emit insecure transport warnings.
- Product, customer, production, shared, and long-running environments must use HTTPS/WSS.
- Agent heartbeat checks the pinned controller fingerprint before opening the WebSocket.
- WebSocket authentication failures are recorded as security audit events.
- Enrollment tokens are not accepted on the task/heartbeat WebSocket channel.

## Task/Data Payloads

task 실행과 결과 전달용 payload:

- `task_assignment`
- `task_ack`
- `task_started`
- `task_rejected`
- `task_cancel`
- `output_chunk`
- `task_result`
- `security_event`
- `facts_snapshot`
- `metrics_snapshot`
- `log_chunk`
- `drift_report`

인증/session payload와 task/data payload는 protocol layer에서 구분된다. agent는 authenticated session 이후에만 task/data channel message를 처리해야 한다.

Facts and metrics payloads include a lightweight system timestamp inside the JSON body so operators can identify when the agent produced the snapshot even after paging or exporting API responses.

- `facts_snapshot` is inventory data. It should describe relatively stable system characteristics such as OS, architecture, hostname, CPU logical count, total memory, memory module count when discoverable, disk device inventory, mount layout, root filesystem, root disk capacity, and network interface names. It must not carry current memory usage, disk usage, or CPU usage.
- `metrics_snapshot` is usage telemetry. It should describe values that change over time, such as CPU usage percent, memory used/available/used percent, disk used/available/used percent, process count, and service failure counts.

Facts snapshot:

```json
{
  "type": "facts_snapshot",
  "payload": {
    "agent_id": "agent-1",
    "body": "{\"system_time_ms\":1710000000000,\"os\":\"linux\",\"arch\":\"x86_64\",\"memory\":{\"total_kb\":16777216,\"module_count_known\":true,\"module_count\":2},\"disk\":{\"device_inventory_known\":true,\"device_count\":1,\"devices\":[{\"name\":\"nvme0n1\",\"kind\":\"disk\",\"size_kb\":104857600,\"removable\":false,\"rotational\":false}],\"mount_inventory_known\":true,\"mount_count\":1,\"mounts\":[{\"source\":\"/dev/nvme0n1p1\",\"mount_point\":\"/\",\"fs_type\":\"ext4\",\"read_only\":false}],\"root_capacity_known\":true,\"root_total_kb\":104857600}}"
  }
}
```

Metrics snapshot:

```json
{
  "type": "metrics_snapshot",
  "payload": {
    "agent_id": "agent-1",
    "body": "{\"system_time_ms\":1710000000000,\"cpu\":{\"logical_count\":4,\"usage_percent\":18.4},\"memory\":{\"usage_available\":true,\"total_kb\":16777216,\"used_kb\":4194304,\"available_kb\":12582912,\"used_percent\":25},\"disk\":{\"usage_available\":true,\"total_kb\":104857600,\"used_kb\":31457280,\"available_kb\":73400320,\"used_percent\":30}}"
  }
}
```

Drift report does not carry an arbitrary JSON body, so controller uses the agent message envelope `timestamp_ms` as both `checked_at_ms` and `agent_system_time_ms`.

Agent operational log chunk:

```json
{
  "type": "log_chunk",
  "payload": {
    "agent_id": "agent-1",
    "line": "level=info event=agent_heartbeat_completed agent_id=agent-1 status=online"
  }
}
```

The default agent start mode sends product-safe operational log chunks every
30 seconds. Operators can change the interval with
`--log-upload-interval-seconds` or disable it with `--disable-log-upload`.
These chunks are not raw file tails or journald streams. They are independent
from heartbeat, facts, metrics, and task assignment dispatch.

## Signed Task Envelope

`task_assignment`는 signed envelope와 실행할 task payload를 포함한다.

```json
{
  "type": "task_assignment",
  "payload": {
    "envelope": {
      "job_id": "job-1",
      "task_id": "task-1",
      "target_agent_id": "agent-1",
      "issued_at_ms": 1,
      "expires_at_ms": 60000,
      "nonce": "nonce-1",
      "payload_hash": "hash",
      "signature": "sig"
    },
    "task": {
      "kind": "command",
      "payload": {
        "program": "uptime",
        "args": [],
        "timeout_ms": 30000,
        "max_output_bytes": 1048576
      }
    }
  }
}
```

Runbook task payload:

```json
{
  "kind": "runbook_execution",
  "payload": {
    "runbook_document": "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\n...",
    "timeout_ms": 180000,
    "confirmed_high_risk": true
  }
}
```

Drift check task payload:

```json
{
  "kind": "drift_check",
  "payload": {
    "policy_document": "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Policy\n..."
  }
}
```

agent는 실행 전에 최소한 다음을 확인해야 한다.

- target agent id가 자기 id와 일치한다.
- signature가 비어 있지 않다.
- expiry가 지나지 않았다.
- nonce replay가 아니다.
- controller public key로 signature를 검증한다.

검증에 실패하면 agent는 task를 실행하지 않고 `task_rejected`를 controller에 보낸다. Controller는 해당 assignment를 `rejected`로 저장하고 job audit event를 남긴다. 별도의 보안 이상 징후나 protocol mismatch는 `security_event`를 통해 Security audit event로 저장한다.

MVP agent는 WebSocket session 안에서 nonce replay guard를 적용한다. Persistent nonce replay store와 장시간 live streaming은 후속 hardening 범위다.

## Assignment Lifecycle, Output, and Result

WebSocket write 성공은 agent가 task를 수락하거나 실행했다는 뜻이 아니다. Controller는 dispatch write 성공 시 assignment를 `dispatched`로만 저장한다.

Agent lifecycle events:

```json
{
  "type": "task_ack",
  "payload": {
    "job_id": "job-1",
    "task_id": "task-1"
  }
}
```

```json
{
  "type": "task_started",
  "payload": {
    "job_id": "job-1",
    "task_id": "task-1"
  }
}
```

```json
{
  "type": "task_rejected",
  "payload": {
    "job_id": "job-1",
    "task_id": "task-1",
    "reason_code": "invalid_signature",
    "reason": "task envelope signature is invalid"
  }
}
```

Rejected reason codes:

- `agent_busy`
- `invalid_signature`
- `expired`
- `replay`
- `target_mismatch`
- `invalid_task`
- `capability_unsupported`
- `local_policy`
- `internal_error`

Controller-to-agent cancel message:

```json
{
  "type": "task_cancel",
  "payload": {
    "job_id": "job-1",
    "task_id": "task-1",
    "reason": "operator requested cancel"
  }
}
```

Agent는 `task_cancel.task_id`가 현재 실행 중인 task와 일치할 때만 cancel을 적용한다. 다른 task id의 cancel은 무시한다. Cancel된 command는 일반 failure가 아니라 `canceled` terminal result로 보고해야 한다.

command/runbook 실행 결과는 application log가 아니라 job output storage로 들어간다.

```json
{
  "type": "output_chunk",
  "payload": {
    "job_id": "job-1",
    "task_id": "task-1",
    "stream": "stdout",
    "sequence": 0,
    "data": "ok"
  }
}
```

```json
{
  "type": "task_result",
  "payload": {
    "job_id": "job-1",
    "task_id": "task-1",
    "exit_code": 0,
    "status": "succeeded",
    "reason": ""
  }
}
```

`output_chunk`는 final result가 아니다. Controller는 output chunk를 job output storage에 저장하지만 assignment를 `succeeded` 또는 `failed`로 바꾸지 않는다.

Controller는 `task_result.status`가 있으면 이를 우선 사용한다. `succeeded`는 assignment `succeeded`와 job `success`, `failed`는 assignment `failed`와 job `failed`, `canceled`는 assignment/job `canceled`, `timed_out`은 assignment/job `expired`로 저장한다. 구버전 agent가 `status` 없이 `task_result`를 보내면 `exit_code == 0`은 success, 그 외는 failed로 fallback 처리한다. 이후 multi-agent fanout에서는 job aggregate 계산이 target별 assignment 결과를 기준으로 확장된다.

이미 `canceled`, `expired`, `failed`, `succeeded`, `rejected` 같은 terminal assignment가 된 뒤에 늦은 `task_result`가 도착하면 Controller는 terminal 상태를 덮어쓰지 않고 `task_result_ignored` audit event만 남긴다. Disconnect나 duplicate session 때문에 늦은 success가 도착해도 canceled job을 success로 바꾸지 않는다.

## Security Event

```json
{
  "type": "security_event",
  "payload": {
    "agent_id": "agent-1",
    "action": "task_verification_failed",
    "detail": "invalid signature"
  }
}
```

`detail`에는 payload 원문이나 secret을 넣지 않는다. 실패 사유 중심의 짧은 문자열만 허용한다.
