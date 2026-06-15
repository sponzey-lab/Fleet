# Sponzey Fleet MVP API

이 문서는 현재 구현된 MVP Controller API 범위를 기록한다. API는 `sponzey controller start`로 실행되는 Controller process가 제공한다.

## Transport

HTTP와 HTTPS controller URL을 모두 허용한다. 단, HTTP는 설치 확인, 로컬
개발, 실험실 테스트, 짧은 검증 용도로만 사용해야 한다.

제품, 고객, 운영, 공동 사용, 장시간 실행 환경에서는 반드시 HTTPS를 사용해야
한다. HTTP는 암호화되지 않으므로 controller/agent 실행 경계에서 경고를
출력하고, controller external URL이 HTTP이면 Security audit에 기록한다.
HTTP 사용으로 발생하는 token 노출, command 탈취, 데이터 유출, 중간자 공격,
기타 위험이 있을 수 있다.

```bash
sponzey controller start --host 127.0.0.1 --port 7700 --data-dir .sponzey --external-url http://127.0.0.1:7700
```

SQLite DB 경로를 명시하려면 bootstrap 시점에 `--db sqlite://...`를 전달한다.

```bash
sponzey controller start --host 127.0.0.1 --port 7700 --data-dir .sponzey --db sqlite:///tmp/sponzey-fleet.db --external-url http://127.0.0.1:7700
```

## Swagger / OpenAPI 지원 기준

Controller가 외부에 제공하는 HTTP API는 Swagger/OpenAPI 문서를 함께 제공한다.
운영자와 외부 자동화 도구가 같은 계약을 보고 연동할 수 있도록, REST API를
추가하거나 변경할 때는 코드, 테스트, `docs/api.md`, OpenAPI 문서를 함께
갱신한다.

제공 endpoint:

```http
GET /openapi.json
GET /swagger-ui
```

- `/openapi.json`은 OpenAPI 3.1 JSON 문서를 반환한다.
- `/swagger-ui`는 브라우저에서 확인할 수 있는 Swagger UI를 제공한다.
- Swagger UI는 Web Admin UI와 별개다. Web Admin UI는 `/admin`, API 문서는 `/swagger-ui`로 접근한다.
- WebSocket agent protocol은 REST OpenAPI 범위에 넣지 않고 `docs/protocol.md`에서 별도로 문서화한다.

OpenAPI 문서 범위:

- `/healthz`
- `/api/controller/identity`
- `/api/agents/enroll`
- `/api/enrollment-tokens`
- `/api/enrollment-tokens/{id}`
- `/api/agents`
- `/api/agents/{agent_id}`
- `/api/agents/{agent_id}/labels`
- `/api/agents/{agent_id}/revoke-key`
- `/api/agents/{agent_id}/facts`
- `/api/agents/{agent_id}/facts/latest`
- `/api/agents/{agent_id}/metrics`
- `/api/agents/{agent_id}/metrics/latest`
- `/api/agents/{agent_id}/drift`
- `/api/agents/{agent_id}/drift/latest`
- `/api/jobs`
- `/api/jobs/command`
- `/api/jobs/runbook`
- `/api/jobs/drift-check`
- `/api/jobs/{job_id}/output`
- `/api/audit`

인증 표기:

- 보호 API는 OpenAPI `bearerAuth` security scheme으로 admin token을 요구한다고 명시한다.
- Swagger UI에서 보호 API를 호출하려면 `Authorize`에 `sponzey controller init`이 출력한 admin token을 넣는다.
- `/api/agents/enroll`은 admin token을 쓰지 않는다. enrollment token은 request body의 `token` 필드로 전달한다.
- `/healthz`, `/api/controller/identity`, `/openapi.json`, `/swagger-ui`는 문서와 readiness 접근을 위해 public endpoint로 둔다.

보안 문서화 규칙:

- OpenAPI example에는 실제 admin token, enrollment token, private key, secret, command output 원문을 넣지 않는다.
- token example은 `<admin-token>`, `<enrollment-token>`, `<redacted>` 같은 placeholder만 사용한다.
- enrollment token create 응답의 raw token은 "생성 직후 1회만 표시되는 민감값"으로 설명한다.
- HTTP transport는 테스트 전용이라는 경고를 Swagger description에도 포함한다.
- Swagger UI를 HTTP endpoint에서 사용할 수는 있지만, HTTP에서는 token과 요청 payload가 암호화되지 않는다. 운영/제품/공동 사용 환경에서는 HTTPS URL의 Swagger UI만 사용해야 한다.

변경 절차:

- 외부 API를 추가하면 OpenAPI path, request schema, response schema, error response를 같이 추가한다.
- 기존 request/response shape를 바꾸면 `info.version`과 package version의 영향도를 확인한다.
- Web Admin UI가 사용하는 API라면 `web-admin/api.schema.json`과 Web Admin smoke test도 함께 갱신한다.
- list API를 새로 추가할 때는 `limit`과 cursor 기반 paging을 우선 사용한다. offset paging은 snapshot이 계속 추가되는 운영 데이터에는 기본값으로 쓰지 않는다.

## Health

```http
GET /healthz
```

응답:

```json
{"status":"ok"}
```

Health endpoint는 인증 없이 접근 가능하다. 이 endpoint는 process가 요청을 받을 수 있는지 확인하기 위한 최소 readiness surface다.

## Controller Identity

```http
GET /api/controller/identity
```

응답:

```json
{
  "controller_public_key": "<ed25519-public-key-hex>",
  "controller_fingerprint": "<sha256-public-key-fingerprint-hex>"
}
```

Agent는 시작 시 저장된 controller fingerprint와 이 응답을 비교한다. 값이 달라지면 explicit re-enroll 없이 연결하지 않는다.

## Admin Token

`sponzey controller init`은 최초 실행 시 admin token을 1회 출력한다.

```bash
sponzey controller init --data-dir .sponzey
```

출력 예:

```text
controller initialized at .sponzey
controller fingerprint: <sha256-public-key-fingerprint-hex>
admin token: admin-...
```

`controller init`은 controller Ed25519 key pair를 생성한다. public key는 `controller_public.key`, private key는 `controller_private.key`에 저장한다. Unix에서는 private key 파일이 group/other에 열려 있으면 init/start path에서 거부한다.

admin token 원문은 DB에 저장하지 않는다. Controller는 token hash만 저장한다. 이미 초기화된 controller에서 다시 `init`을 실행하면 controller key와 admin token을 새로 만들지 않는다.

MVP에서 Controller와 CLI가 새로 생성하는 token, job, assignment nonce, message id는 사람이 읽을 수 있는 prefix와 ULID를 조합한 형태를 사용한다. 예: `admin-...`, `enroll-...`, `et-...`, `job-cli-...`, `nonce-...`, `msg-...`.

보호 API는 다음 header를 요구한다.

```http
Authorization: Bearer <admin-token>
```

## Enrollment Token API

### Create

```http
POST /api/enrollment-tokens
Authorization: Bearer <admin-token>
```

요청 body는 비워도 되며, 비어 있으면 `max_uses=1`, `expires_in_seconds=3600`, empty labels를 기본값으로 사용한다.

```json
{
  "labels": "role=web,env=prod",
  "max_uses": 1,
  "expires_in_seconds": 3600
}
```

응답은 raw enrollment token을 1회 포함한다.

```json
{"id":"et-...","token":"enroll-...","expires_in_seconds":3600}
```

raw enrollment token은 DB에 저장하지 않는다. DB에는 token hash와 metadata만 저장한다.
생성 이벤트는 enrollment audit에 남기되 raw token은 기록하지 않고 token id reference만 남긴다.

### List

```http
GET /api/enrollment-tokens
Authorization: Bearer <admin-token>
```

응답:

```json
[
  {
    "id": "et-...",
    "default_labels": "",
    "max_uses": 1,
    "used_count": 0,
    "remaining_uses": 1,
    "revoked": false,
    "expires_at_epoch": 1710003600
  }
]
```

### Revoke

```http
DELETE /api/enrollment-tokens/{id}
Authorization: Bearer <admin-token>
```

성공 시 `204 No Content`를 반환한다.
폐기 이벤트도 enrollment audit에 남긴다.

## Agent Enrollment API

Agent는 enrollment token을 사용해 controller에 자기 identity를 등록한다. 이 endpoint는 admin token을 사용하지 않는다.

```http
POST /api/agents/enroll
Content-Type: application/json
```

요청:

```json
{
  "token": "enroll-...",
  "agent_id": "agent-web-01",
  "name": "web-01",
  "public_key": "<ed25519-public-key-hex>",
  "fingerprint": "<sha256-public-key-fingerprint-hex>",
  "labels": [
    {"key": "role", "value": "web"}
  ]
}
```

응답:

```json
{
  "agent_id": "agent-web-01",
  "controller_public_key": "<ed25519-public-key-hex>",
  "controller_fingerprint": "<sha256-public-key-fingerprint-hex>"
}
```

Controller는 raw enrollment token을 hash로 검증하고, 성공 시 token use count를 증가시킨다. 만료, 폐기, max uses 초과 token은 거부한다. 등록 시 public key와 fingerprint가 일치하지 않으면 거부한다. Enrollment token에 default labels가 있으면 agent labels에 적용하고, agent가 명시한 같은 key의 label은 default를 override한다.

## Command Job API

MVP의 command job API는 admin token 인증 후 command job과 controller-signed task assignment를 생성한다. 등록된 agent가 다음 heartbeat WebSocket session을 열면 controller는 queued assignment 하나를 dispatch하고, agent는 command를 실행한 뒤 output chunk와 task result를 controller에 돌려보낸다. 실행 중 실시간 streaming과 별도 output subscribe API는 아직 후속 범위다.

```http
POST /api/jobs/command
Authorization: Bearer <admin-token>
Content-Type: application/json
```

요청:

```json
{
  "job_id": "job-1",
  "target_agent_ids": ["agent-web-01"],
  "selector": null,
  "program": "uptime",
  "args": [],
  "timeout_seconds": 30,
  "confirmed_high_risk": true,
  "confirmed_by": "operator@example.com",
  "expires_in_seconds": 60,
  "nonce_prefix": "nonce-job-1"
}
```

응답:

```json
{
  "job_id": "job-1",
  "target_count": 1,
  "assignment_count": 1
}
```

`target_agent_ids`를 명시하면 해당 agent를 대상으로 한다. `target_agent_ids`가 비어 있고 `selector`가 있으면 controller가 inventory에서 matching agent를 찾아 target을 만든다. selector 예시는 `role=web,env=prod`, `agent:web-01`이다.

command task는 기본 high-risk로 분류된다. `confirmed_high_risk`가 `false`이면 job 생성 전에 거부한다. Controller는 private signing key로 task envelope signature를 만들고, job과 task assignment를 SQLite에 저장한 뒤 `job_created` audit event를 남긴다. `confirmed_by`는 high-risk 확인 actor로 audit에 남긴다. Dispatch 시 `job_started`, result 수신 시 `job_completed` 또는 `job_failed` audit event도 남긴다.

CLI에서 controller API로 job을 생성하려면 admin token을 명시 인자로 전달한다. token은 command payload나 job output에 섞지 않는다.

```bash
sponzey run \
  --controller-url http://127.0.0.1:7700 \
  --admin-token <admin-token> \
  --selector role=web \
  --confirm-risk \
  uptime
```

## Runbook Job API

Runbook job API는 admin token 인증 후 runbook 문서를 validation하고, controller-signed task assignment를 생성한다. 실제 package/service/file primitive 실행은 agent가 signed envelope를 검증한 뒤 수행한다. `sponzey apply`는 local validation-only 명령이며, privileged execution path가 아니다.

```http
POST /api/jobs/runbook
Authorization: Bearer <admin-token>
Content-Type: application/json
```

요청:

```json
{
  "job_id": "job-nginx-runbook-1",
  "target_agent_ids": [],
  "selector": "role=web",
  "runbook_document": "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nmetadata:\n  name: nginx-basic\nspec:\n  targets:\n    selector: role=web\n  tasks:\n    - id: nginx-package\n      package:\n        name: nginx\n        state: present\n",
  "timeout_seconds": 180,
  "confirmed_high_risk": true,
  "confirmed_by": "operator@example.com",
  "expires_in_seconds": 300,
  "nonce_prefix": "nonce-nginx-runbook"
}
```

응답:

```json
{
  "job_id": "job-nginx-runbook-1",
  "target_count": 1,
  "assignment_count": 1
}
```

규칙:

- invalid runbook은 task assignment 생성 전에 거부한다.
- runbook execution task는 high-risk로 분류한다.
- `confirmed_high_risk`가 `false`이면 거부한다.
- selector resolution은 disabled agent를 제외한다.
- agent는 signed envelope 검증, expiry 검증, replay 검증 이후에만 실행한다.
- step output은 job output chunk로 저장하고 Product application log와 분리한다.

### List Jobs

Web Admin UI와 CLI 확인용으로 최근 job summary를 조회한다.

```http
GET /api/jobs
Authorization: Bearer <admin-token>
```

응답:

```json
[
  {
    "id": "job-1",
    "status": "queued",
    "risk": "high",
    "command_program": "uptime",
    "command_args": ["-a"],
    "target_count": 1,
    "created_at_ms": 1710000000000
  }
]
```

이 API는 저장된 summary만 보여준다. authorization 판단이나 job 상태 전이는 controller application/domain 경계에서 처리한다.

### Poll Output

MVP는 실시간 subscribe 대신 polling 방식 output 조회 API를 제공한다.

```http
GET /api/jobs/{job_id}/output
Authorization: Bearer <admin-token>
```

응답:

```json
[
  {
    "job_id": "job-1",
    "agent_id": "agent-web-01",
    "stream": "stdout",
    "sequence": 0,
    "data": "ok"
  }
]
```

이 API는 job output storage를 조회한다. command stdout/stderr는 Product application log에 자동 기록하지 않는다.

## Agent Inventory API

### List

```http
GET /api/agents
Authorization: Bearer <admin-token>
```

응답:

```json
[
  {
    "id": "agent-web-01",
    "name": "web-01",
    "status": "online",
    "revoked": false,
    "fingerprint": "<agent-fingerprint>",
    "labels": [
      {"key": "role", "value": "web"}
    ],
    "last_seen_at_ms": 1710000000000,
    "last_seen_age_seconds": 12,
    "hostname": "web-01",
    "os": "linux",
    "arch": "x86_64"
  }
]
```

`hostname`, `os`, `arch`는 최신 facts snapshot에서 추출한 얇은 inventory summary다. facts가 아직 없으면 `null`이다. `last_seen_age_seconds`는 response 생성 시점 기준의 health 판단 보조값이며, `last_seen_at_ms`가 없으면 `null`이다.

Agent key가 revoke되어 더 이상 heartbeat를 받아들이면 안 되는 agent는 inventory에서 `"status": "offline"`과 `"revoked": true`를 함께 반환한다. 내부 저장 상태는 disabled/revoked로 분리될 수 있지만, 운영 화면에서는 연결 불가 상태와 revoke 상태가 동시에 드러나야 한다.

### Detail

```http
GET /api/agents/{agent_id}
Authorization: Bearer <admin-token>
```

응답은 list item과 같은 shape의 단일 object다. 존재하지 않는 agent는 `404`를 반환한다. Agent public key 원문은 이 API에 노출하지 않는다.

### Revoke Agent Key

```http
POST /api/agents/{agent_id}/revoke-key
Authorization: Bearer <admin-token>
```

Agent key를 revoke하고 agent를 disabled 상태로 전환한다. 응답은 갱신된 agent detail object이며, 운영 화면에서는 `"status": "offline"`과 `"revoked": true`가 함께 표시된다. 이후 같은 key를 사용하는 WebSocket 인증과 heartbeat online 전환은 허용되지 않는다. 존재하지 않는 agent는 `404`를 반환한다. 성공 시 `agent_key_revoked` audit event를 남긴다.

### Update Labels

```http
PATCH /api/agents/{agent_id}/labels
Authorization: Bearer <admin-token>
Content-Type: application/json
```

요청:

```json
{
  "labels": [
    {"key": "role", "value": "api"},
    {"key": "env", "value": "prod"}
  ]
}
```

응답은 갱신된 agent detail object다. Label key/value는 domain validation을 통과해야 한다. 성공 시 `agent_labels_updated` audit event를 남기며, audit에는 label 원문 전체 대신 label count 중심 metadata를 기록한다.

### Latest Facts

```http
GET /api/agents/{agent_id}/facts/latest
Authorization: Bearer <admin-token>
```

응답:

```json
{
  "agent_id": "agent-web-01",
  "collected_at_ms": 1710000000000,
  "agent_system_time_ms": 1710000000000,
  "body": {
    "system_time_ms": 1710000000000,
    "os": "linux",
    "arch": "x86_64",
    "family": "unix",
    "cpu": {
      "logical_count": 4
    },
    "memory": {
      "total_kb": 16384256,
      "module_count_known": true,
      "module_count": 2,
      "module_count_source": "linux_dmi_type17"
    },
    "network": {
      "interfaces": ["lo", "eth0"]
    },
    "disk": {
      "root_mount_known": true,
      "root_capacity_known": true,
      "root_filesystem": "/dev/root",
      "root_total_kb": 52428800
    },
    "degraded": {
      "status": false,
      "signals": []
    }
  }
}
```

Agent facts snapshot은 heartbeat session에서 전송된다. `collected_at_ms`는 controller가 저장한 snapshot 시각이며, 신규 agent message에서는 agent가 보낸 message timestamp를 기준으로 한다. `agent_system_time_ms`는 해당 snapshot을 만든 agent 시스템 기준 시각이다. Facts/metrics payload 내부의 `body.system_time_ms`도 동일한 agent 시스템 시각을 담는다. Facts는 OS, architecture, platform family, hostname, CPU logical count, memory total/module count, Linux `/proc/net/dev` 기반 network interface, root disk capacity 같은 비교적 변하지 않는 inventory만 담는다. 현재 메모리 사용량, 디스크 사용량, CPU 사용률은 facts가 아니라 metrics에 담는다. Facts payload의 `degraded.status=true`는 controller에서 agent 상태 `degraded`로 반영된다.

### Facts Snapshot Pages

```http
GET /api/agents/{agent_id}/facts?limit=50&before=<cursor>
Authorization: Bearer <admin-token>
```

응답:

```json
{
  "items": [
    {
      "agent_id": "agent-web-01",
      "collected_at_ms": 1710000000000,
      "agent_system_time_ms": 1710000000000,
      "body": {"system_time_ms": 1710000000000, "os": "linux"},
      "cursor": "1710000000:42"
    }
  ],
  "next_cursor": "1710000000:42"
}
```

`limit` 기본값은 50이고 최대 500이다. `before`는 이전 응답의
`next_cursor` 값을 그대로 넣는다. Cursor는 opaque value로 취급하고
클라이언트에서 분해하거나 직접 만들지 않는다. 응답은 최신 snapshot부터
내림차순으로 반환한다. 다음 페이지가 있으면 `next_cursor`를 반환하고,
더 가져올 row가 없으면 `next_cursor`는 `null`이다.

### Latest Metrics

```http
GET /api/agents/{agent_id}/metrics/latest
Authorization: Bearer <admin-token>
```

응답:

```json
{
  "agent_id": "agent-web-01",
  "collected_at_ms": 1710000000000,
  "agent_system_time_ms": 1710000000000,
  "body": {
    "system_time_ms": 1710000000000,
    "cpu": {
      "logical_count": 4,
      "usage_percent": 18.4
    },
    "memory": {
      "usage_available": true,
      "total_kb": 16384256,
      "used_kb": 8260800,
      "available_kb": 8123456,
      "used_percent": 50
    },
    "process": {
      "pid": 1234,
      "count": 92
    },
    "service": {
      "status_available": true,
      "failed_units_count": 0,
      "failed_units": []
    },
    "disk": {
      "usage_available": true,
      "total_kb": 52428800,
      "used_kb": 18432000,
      "available_kb": 33996800,
      "used_percent": 35
    }
  }
}
```

Metrics snapshot도 heartbeat session에서 전송된다. `collected_at_ms`는 저장된 snapshot 시각이고, `agent_system_time_ms`는 agent가 metrics를 만든 시스템 시각이다. Metrics는 CPU 사용률, 메모리 사용량/사용률, 디스크 사용량/사용률, process count, service failure count처럼 시간에 따라 변하는 사용량 telemetry를 담는다. MVP는 lightweight snapshot만 저장하며 time-series observability platform으로 확장하지 않는다. `service.status_available=false`는 systemd가 없거나 조회가 불가능한 환경을 의미하며, collector 실패로 process를 중단하지 않는다. Retention cleanup은 `sponzey retention cleanup`으로 명시적으로 실행한다.

### Metrics Snapshot Pages

```http
GET /api/agents/{agent_id}/metrics?limit=50&before=<cursor>
Authorization: Bearer <admin-token>
```

응답:

```json
{
  "items": [
    {
      "agent_id": "agent-web-01",
      "collected_at_ms": 1710000000000,
      "agent_system_time_ms": 1710000000000,
      "body": {"system_time_ms": 1710000000000, "cpu": {"logical_count": 4}},
      "cursor": "1710000000:42"
    }
  ],
  "next_cursor": "1710000000:42"
}
```

Paging 규칙은 facts snapshot pages와 동일하다. `before`는 이전 응답의
`next_cursor`를 그대로 사용한다.

### Latest Drift Report

```http
GET /api/agents/{agent_id}/drift/latest
Authorization: Bearer <admin-token>
```

응답:

```json
{
  "agent_id": "agent-web-01",
  "checked_at_ms": 1710000000000,
  "agent_system_time_ms": 1710000000000,
  "policy_name": "nginx-running",
  "status": "drifted",
  "expected": "service nginx running",
  "actual": "stopped"
}
```

Agent가 WebSocket task-data channel로 보낸 drift report는 `drift_reports`에 저장되고 `drift_report_received` audit event를 남긴다. `checked_at_ms`와 `agent_system_time_ms`는 agent가 drift report message를 보낸 시스템 시각을 기준으로 한다. Local `sponzey drift check --policy`는 service running, package present, file SHA-256 check engine을 사용한다. 다만 controller가 signed drift job을 agent에 dispatch하는 흐름은 아직 후속 범위다.

### Drift Report Pages

```http
GET /api/agents/{agent_id}/drift?limit=50&before=<cursor>
Authorization: Bearer <admin-token>
```

응답:

```json
{
  "items": [
    {
      "agent_id": "agent-web-01",
      "checked_at_ms": 1710000000000,
      "agent_system_time_ms": 1710000000000,
      "policy_name": "nginx-running",
      "status": "drifted",
      "expected": "service nginx running",
      "actual": "stopped",
      "cursor": "1710000000:42"
    }
  ],
  "next_cursor": "1710000000:42"
}
```

Paging 규칙은 facts/metrics snapshot pages와 동일하다.

### Audit Events

```http
GET /api/audit
Authorization: Bearer <admin-token>
```

응답:

```json
[
  {
    "category": "security",
    "action": "invalid_signature",
    "actor": "system",
    "target": "agent-web-01",
    "value_kind": "redacted",
    "value": "redacted",
    "occurred_at_ms": 1710000000000
  }
]
```

Audit API는 최근 50개 event를 최신순으로 반환한다. `SecretRef` 값은 원문을 반환하지 않고 `secret_ref` marker로만 노출한다.

## Current Limits

- Axum/Actix 같은 production-grade HTTP framework로 아직 전환하지 않았다.
- Controller accept loop는 명시적 shutdown signal 경계를 갖지만, process signal integration은 CLI/runtime 후속 작업이다.
- controller key pair rotation은 아직 구현하지 않았다.
- admin token CLI profile 저장 방식은 아직 구현하지 않았다.
- WebSocket heartbeat 이후 queued command assignment dispatch와 completed output/result 수신은 동작한다. Web Admin UI는 command job 생성과 polling 기반 output viewer를 제공한다. CLI live renderer, true streaming subscribe, multi-agent fan-out 상태 집계는 후속 task 범위다.
