# Task 007 - Job 상태와 Output UX 정밀화

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

운영자가 job이 어디서 멈췄는지 Web Admin에서 알 수 있게 한다.

persistent session이 도입되면 단순히 "No job output"만으로는 충분하지 않다. job created, queued, delivered, running, completed, rejected, expired를 구분해야 한다.

## 기능 범위

### 1. Job/detail API 보강

- [x] `/api/jobs` 응답 또는 신규 `/api/jobs/{job_id}` 응답에 dispatch 상태를 포함한다.
- [x] target agent 상태와 connected 여부를 API로 제공한다.
- [x] Web Admin이 connected/running 상태를 자체 추측하지 않도록 한다.

응답 후보:

```json
{
  "id": "job-1",
  "status": "running",
  "dispatch_state": "delivered",
  "target_agent_ids": ["agent-1"],
  "created_at_ms": 1710000000000,
  "updated_at_ms": 1710000001000,
  "expires_at_ms": 1710000060000,
  "last_error": ""
}
```

체크:

- [x] status와 dispatch_state 의미가 섞이지 않는다.
- [x] raw command output은 job detail에 넣지 않는다.
- [x] OpenAPI와 `web-admin/api.schema.json`을 함께 갱신한다.

### 2. Web Admin Run 패널 상태 문구 분리

- [x] job created: `Job created. Checking dispatch state.`
- [x] queued/offline: `Queued until agent reconnects.`
- [x] delivered/running: `Running on agent. Waiting for output.`
- [x] output streaming: chunk count와 sequence를 표시한다.
- [x] completed without output: `Completed with no output.`
- [x] rejected/expired/failed: 원인과 다음 조치를 보여준다.

규칙:

- `No job output`은 polling 완료 후에도 실제 chunk가 전혀 없을 때만 표시한다.
- pending 상태에서 `No job output`을 표시하지 않는다.
- UI는 domain rule을 재구현하지 않고 API 상태를 표현한다.

### 3. Output polling 개선

- [x] `/api/jobs/{job_id}/output` polling과 job status polling을 함께 수행한다.
- [x] job이 terminal 상태가 되면 polling을 멈춘다.
- [x] connected/running 상태인데 output이 없는 경우와 completed no-output을 구분한다.

후속 후보:

- Admin UI용 SSE 또는 WebSocket output subscribe
- 현재 task에서는 REST polling fallback을 안정화하는 것을 우선한다.

## 테스트와 검증

필수:

- [x] job detail API test
- [x] job status transition response test
- [x] Web Admin smoke test
- [x] output 없는 completed job과 pending job 문구 구분 test
- [x] rejected/expired 표시 test
- [x] OpenAPI와 `web-admin/api.schema.json` 정합성 test
- [x] `npm test --workspace web-admin`
- [x] `npm run typecheck --workspace web-admin`
- [x] `npm run build --workspace web-admin`
- [x] `cargo test -p fleet-controller jobs`
- [x] `git diff --check`

## 완료 기준

- [x] 운영자가 queued/offline/running/completed/rejected/expired를 구분할 수 있다.
- [x] `No job output`이 pending 상태에서 보이지 않는다.
- [x] Web Admin은 상태를 API에서 받아 표시한다.
- [x] API 문서와 Swagger가 실제 응답과 일치한다.

## 비범위

- [x] Admin streaming subscribe 구현하지 않음
- [x] 복잡한 dashboard builder 만들지 않음
- [x] UI에서 authorization 판단하지 않음
