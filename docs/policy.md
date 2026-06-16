# Policy Assignment and Scheduled Drift

이 문서는 Sponzey Fleet의 policy 기반 drift 운영 루프를 설명한다.

## Scope

현재 구현 범위는 다음과 같다.

- Policy 문서를 저장한다.
- Policy를 agent에 직접 배정한다.
- Agent inventory API에서 배정된 policy id를 확인한다.
- Scheduled drift check 대상과 다음 실행 시각을 저장한다.
- Drift report는 latest와 history API로 구분해서 조회한다.
- Drift report는 severity와 acknowledgement/resolution 상태를 가진다.
- Remediation은 자동 실행하지 않고 approval workflow로만 연결한다.

이번 범위에서 제외한 것:

- group/selector 자동 확장 assignment worker
- scheduled drift worker가 실제 drift-check job을 자동 생성하는 루프
- 자동 remediation 실행
- Web Admin policy management 화면

## Policy Object

Policy 문서는 Rust domain layer에서 검증한다. Controller API는 policy source를
문자열로 받고, 저장 전에 `parse_policy_document`를 통과시킨다.

최소 예:

```yaml
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  id: policy-nginx-running
  name: nginx-running
  version: 1
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: nginx-service
      service:
        name: nginx
        state: running
  schedule:
    intervalSeconds: 300
  remediation:
    runbookRef: runbooks/nginx-restart.yml
    approvalRequired: true
```

Policy identity:

- `metadata.id`: 저장 key. 없으면 `metadata.name`을 사용한다.
- `metadata.name`: drift report의 사람이 읽는 policy 이름.
- `metadata.version`: 양의 정수. 없으면 `1`.
- `source`: 저장된 원문 policy document.

Validation:

- `apiVersion`과 `kind: Policy`가 필요하다.
- `spec.selector.matchLabels`가 필요하다.
- `spec.checks`가 최소 1개 필요하다.
- 현재 check primitive는 service running, package present, file SHA-256이다.
- remediation이 선언되면 MVP에서는 `approvalRequired: true`가 필요하다.

## Assignment

현재 제품 경계는 direct agent assignment다.

```http
POST /api/policies/{policy_id}/assignments
Authorization: Bearer <admin-token>

{"agent_id":"agent-1"}
```

응답:

```json
{
  "policy_id": "policy-nginx-running",
  "agent_id": "agent-1",
  "assigned_at_ms": 1710000000000
}
```

Agent inventory 응답에는 `assigned_policy_ids`가 포함된다. UI나 외부 자동화는
agent 목록에서 현재 배정된 policy id를 확인할 수 있다.

Selector 기반 assignment는 domain type으로 방향만 열어두었고, 실제 rollout
worker와 UI는 후속 task 범위다.

## Scheduled Drift

Schedule은 policy와 agent pair에 저장된다.

```http
POST /api/policies/{policy_id}/schedules
Authorization: Bearer <admin-token>

{"agent_id":"agent-1","interval_seconds":300}
```

응답:

```json
{
  "policy_id": "policy-nginx-running",
  "agent_id": "agent-1",
  "interval_seconds": 300,
  "next_due_at_ms": 1710000300000,
  "last_checked_at_ms": null
}
```

Due schedule 조회:

```http
GET /api/drift/scheduled
Authorization: Bearer <admin-token>
```

현재 구현은 due schedule을 조회하고 저장소에서 `last_checked_at`와 다음
`next_due_at`를 갱신할 수 있는 repository 경계를 제공한다. 실제 background
worker가 due schedule을 읽어 drift-check job을 자동 생성하는 기능은 후속
작업이다.

Missed schedule handling:

- Domain helper는 `next_due_at`보다 현재 시간이 늦으면 due로 판단한다.
- grace duration보다 늦은 schedule은 `missed=true`로 표시할 수 있다.
- worker 구현 시 missed schedule은 audit 대상이다.

## Drift Latest and History

Drift는 latest와 history를 구분한다.

- `GET /api/agents/{agent_id}/drift/latest`: 가장 최근 report 하나.
- `GET /api/agents/{agent_id}/drift?limit=50&before=<cursor>`: cursor 기반 history.

Drift report fields:

- `status`: `compliant`, `drifted`, `unknown`
- `severity`: `none`, `warning`, `critical`, `unknown`
- `acknowledged`: operator가 확인했거나 remediation result로 resolved 된 상태
- `resolved`: remediation result가 latest drift를 해결한 상태
- `checked_at_ms`: controller가 저장한 drift report 시각
- `agent_system_time_ms`: agent가 drift report message를 만든 시각

현재 WebSocket drift report는 status에 따라 severity를 기본 매핑한다.

| Status | Default severity |
| --- | --- |
| `compliant` | `none` |
| `drifted` | `warning` |
| `unknown` | `unknown` |

`critical` severity는 policy/rule severity 모델이 확장될 때 사용한다.

## Remediation

Remediation은 자동 실행하지 않는다.

현재 정책:

- Policy에 remediation이 선언되면 `approvalRequired: true`가 필요하다.
- Drift에서 remediation request를 만들 수 있다.
- Remediation request는 approval request로 연결된다.
- Approval 없이 runbook을 자동 실행하지 않는다.
- Remediation job 결과가 성공하면 latest drift report를 resolved 상태로 연결할 수 있다.

이 정책은 root 권한 실행, 운영 변경, audit 요구사항 때문에 의도적으로 보수적이다.

## API Summary

```http
GET  /api/policies
POST /api/policies
POST /api/policies/{policy_id}/assignments
POST /api/policies/{policy_id}/schedules
GET  /api/agents/{agent_id}/policies
GET  /api/drift/scheduled
GET  /api/agents/{agent_id}/drift/latest
GET  /api/agents/{agent_id}/drift
```

모든 policy와 scheduled drift API는 admin token을 요구한다.

## Audit

다음 이벤트가 audit에 남는다.

- `policy_saved`
- `policy_assigned`
- `scheduled_drift_configured`
- `remediation_approval_requested`
- `drift_resolved_by_remediation`

Scheduled drift worker가 실제 자동 job 생성 루프를 갖게 되면 missed schedule,
job creation, failure도 audit 대상이 된다.
