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
6. Agent sends `heartbeat`.
7. Controller updates `last_seen_at` and marks the agent `online`.
8. If one queued assignment exists for the agent, Controller sends `task_assignment`.
9. Agent verifies the signed envelope and executes the command, drift check, or runbook task.
10. Agent sends `output_chunk` messages and one `task_result`.

Agent enrollment generates an Ed25519 key pair locally. The private key is stored in `agent_private.key`; the controller stores the public key and fingerprint. On Unix, `agent.conf` and `agent_private.key` must not be readable, writable, or executable by group/other.

`sponzey agent start` runs as a heartbeat loop by default. Controller connection failures are retried indefinitely unless `--once` or `--max-reconnect-attempts` is set. For smoke tests and one-shot checks, pass `--once`.

Security notes:

- HTTP/WebSocket controller URLs are test-only and emit insecure transport warnings.
- Product, customer, production, shared, and long-running environments must use HTTPS/WSS.
- Agent heartbeat checks the pinned controller fingerprint before opening the WebSocket.
- WebSocket authentication failures are recorded as security audit events.
- Enrollment tokens are not accepted on the task/heartbeat WebSocket channel.

## Task/Data Payloads

task 실행과 결과 전달용 payload:

- `task_assignment`
- `output_chunk`
- `task_result`
- `security_event`
- `facts_snapshot`
- `metrics_snapshot`
- `log_chunk`
- `drift_report`

인증/session payload와 task/data payload는 protocol layer에서 구분된다. agent는 authenticated session 이후에만 task/data channel message를 처리해야 한다.

Facts and metrics payloads include a lightweight system timestamp inside the JSON body so operators can identify when the agent produced the snapshot even after paging or exporting API responses.

- `facts_snapshot` is inventory data. It should describe relatively stable system characteristics such as OS, architecture, hostname, CPU logical count, total memory, memory module count when discoverable, root filesystem, root disk capacity, and network interface names. It must not carry current memory usage, disk usage, or CPU usage.
- `metrics_snapshot` is usage telemetry. It should describe values that change over time, such as CPU usage percent, memory used/available/used percent, disk used/available/used percent, process count, and service failure counts.

Facts snapshot:

```json
{
  "type": "facts_snapshot",
  "payload": {
    "agent_id": "agent-1",
    "body": "{\"system_time_ms\":1710000000000,\"os\":\"linux\",\"arch\":\"x86_64\",\"memory\":{\"total_kb\":16777216,\"module_count_known\":true,\"module_count\":2},\"disk\":{\"root_capacity_known\":true,\"root_total_kb\":104857600}}"
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
These chunks are not raw file tails or journald streams.

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

검증에 실패하면 agent는 task를 실행하지 않고 `security_event`를 controller에 보낸다. Controller는 이를 Security audit event로 저장한다.

MVP agent는 WebSocket session 안에서 nonce replay guard를 적용한다. Persistent nonce replay store와 장시간 live streaming은 후속 hardening 범위다.

## Output and Result

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
    "exit_code": 0
  }
}
```

Controller는 `exit_code == 0`이면 job을 `success`, 아니면 `failed`로 저장한다.

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
