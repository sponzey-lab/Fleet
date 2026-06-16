# Task 006 - 즉시 Task Dispatch

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

`Run` 요청 직후 connected Agent에 task를 즉시 전달한다.

이 task가 완료되면 heartbeat interval이 30초여도, Agent가 이미 connected 상태라면 다음 heartbeat를 기다리지 않고 task_assignment가 전달되어야 한다.

## 기능 범위

### 1. Job 생성 직후 active session dispatch

- [x] `POST /api/jobs/command` 후 저장된 assignment를 active session으로 dispatch한다.
- [x] `POST /api/jobs/runbook`, `POST /api/jobs/drift-check`도 같은 dispatch service를 사용한다.
- [x] connected session이 없으면 queued 상태로 유지한다.

필수 순서:

```text
1. request validation
2. job/assignment DB 저장
3. active session 조회
4. task_assignment send 시도
5. send 성공이면 상태/audit 갱신
6. send 실패이면 queued 유지 또는 dispatch failure 기록
```

금지:

- [x] DB 저장 전 WebSocket send가 없는지 확인한다.
- [x] API handler에 dispatch rule 직접 중복 구현이 없는지 확인한다.
- [x] active session이 있다는 이유로 approval/high-risk 정책을 우회하지 않는지 확인한다.

### 2. Agent reconnect 후 pending queue drain

- [x] Agent session 등록 직후 pending assignment를 조회한다.
- [x] pending assignment가 있으면 즉시 dispatch한다.
- [x] agent당 concurrent task 1개 정책을 지킨다.

정책:

- expired assignment는 보내지 않는다.
- disabled/revoked agent에는 보내지 않는다.
- send 실패 시 queued 유지 또는 failure audit를 남긴다.
- 현재 store query가 type별이면 bias를 줄이기 위한 통합 query를 우선 검토한다.

### 3. Dispatch audit와 latency 기록

- [x] `task_dispatched` audit를 남긴다.
- [x] `task_dispatch_failed` audit를 남긴다.
- [x] Product log에는 job id, agent id, status, latency 중심으로 남긴다.
- [x] command output 원문은 log에 남기지 않는다.

필드 후보:

```text
job_id
agent_id
task_id
dispatch_state
dispatch_latency_ms
active_session=true/false
failure_reason
```

## 테스트와 검증

필수:

- [x] connected agent에 command job 즉시 dispatch integration test
- [x] disconnected agent job queued test
- [x] send 실패 시 queued 유지 또는 dispatch failure audit test
- [x] reconnect 후 queued job drain test
- [x] revoked agent에는 dispatch하지 않는 test
- [x] high-risk confirmation/approval 유지 test
- [x] `cargo test -p fleet-controller dispatch`
- [x] `cargo test -p fleet-application dispatch`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

Smoke:

- [x] heartbeat interval을 30초로 두고도 connected Agent에서 `Run` 후 즉시 output이 시작되는 smoke를 작성한다.

## 완료 기준

- [x] connected Agent는 `Run` 직후 task_assignment를 받는다.
- [x] disconnected Agent는 queued 상태로 유지되고 reconnect 후 받는다.
- [x] dispatch success/failure가 audit로 남는다.
- [x] Web Admin의 지연은 command 실행 시간과 output polling 시간으로만 제한된다.

## 비범위

- [x] Admin UI streaming subscribe 구현하지 않음
- [x] multi-agent fan-out 고도화하지 않음
- [x] task cancellation 구현하지 않음
