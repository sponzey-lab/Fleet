# Task 002 - Domain/Application/Store Dispatch 계약 정리

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

persistent session 구현 전에 job/assignment 상태와 dispatch use case 계약을 먼저 고정한다.

상시 연결의 핵심은 "connected Agent에게 즉시 task를 push"하는 것이지만, source of truth는 DB의 queued job/assignment다. 이 task는 WebSocket session 구현이 application/domain/store 경계를 침범하지 않도록 계약을 먼저 정리한다.

## 배경

현재 사실:

- `task_assignments` 테이블은 이미 존재한다.
- 현재 `task_assignments`에는 status, dispatched_at, started_at, completed_at, last_error가 없다.
- `jobs.status` domain enum에는 `Queued`, `Running`, `Success`, `Failed`, `Canceled`, `Expired` 등이 있다.
- `Dispatched`, `Accepted`, `Rejected`는 아직 domain 상태에 없다.
- output chunk는 `(job_id, agent_id, stream, chunk_index)` unique constraint를 가진다.

핵심 판단:

- DB에 job/assignment를 저장하기 전에 WebSocket으로 먼저 보내면 안 된다.
- WebSocket send와 DB status update는 하나의 원자적 transaction이 될 수 없다.
- API/WebSocket handler가 dispatch business rule을 직접 갖지 않아야 한다.

## 기능 범위

### 1. 1차 job/assignment 상태 모델 결정

- [x] 현재 `JobStatus`에서 1차 구현에 쓸 상태를 결정한다.
- [x] `send success`를 바로 `running`으로 볼지, 별도 dispatch_state를 둘지 결정한다.
- [x] `Rejected`를 domain `JobStatus`에 추가할지, assignment state 또는 audit로만 표현할지 결정한다.

권장 1차 결정:

- job은 기존 `Queued -> Running -> Success/Failed/Expired/Canceled`를 우선 유지한다.
- WebSocket send 성공은 1차에서 coarse하게 `Running`으로 표현할 수 있다.
- `task_ack`/`task_started` payload를 도입하기 전까지 `Dispatched`와 `Running`은 엄밀히 분리하지 않는다.
- 다만 API 문서에는 `running`이 "delivered to active session or executing" 수준의 coarse 상태일 수 있음을 명시한다.

체크:

- [x] domain state transition 테스트가 현재 정책과 맞는다.
- [x] `JobStatus` 확장 시 store parse/string mapping도 같이 갱신한다.
- [x] Web Admin 표시 문구가 domain 상태보다 과하게 단정하지 않는다.

### 2. Dispatch use case 계약 정의

- [x] `DispatchPendingAssignments` application service contract를 정의한다.
- [x] input은 `agent_id`, `job_id`, 또는 둘 다를 받을 수 있게 설계한다.
- [x] active session 유무는 application input 또는 trait으로 전달하고, domain이 WebSocket을 알지 않게 한다.

권장 계약:

```text
DispatchPendingAssignments
  input:
    agent_id: Option<AgentId>
    job_id: Option<JobId>
    now
  dependencies:
    Job/Assignment repository
    Agent inventory repository
    Session dispatcher trait
    Audit writer
  output:
    dispatched_count
    queued_count
    skipped_expired_count
    failed_count
```

필수 정책:

- [x] disabled/revoked agent에는 dispatch하지 않는다.
- [x] expired assignment는 보내지 않는다.
- [x] connected session이 없으면 queued 상태로 둔다.
- [x] send 실패 시 queued 유지 또는 dispatch failure로 기록한다.
- [x] audit 누락 없이 success/failure를 남긴다.

### 3. Store/repository 계약 보강

- [x] pending assignment를 type별이 아니라 created_at/FIFO 기준으로 조회할 수 있는 query 필요 여부를 결정한다.
- [x] 현재 type별 pending query를 유지한다면 bias를 문서화하고 테스트로 고정한다.
- [x] output chunk duplicate policy를 결정한다.

필요할 수 있는 repository method:

```text
list_pending_assignments_for_agent(agent_id, limit)
mark_job_running(job_id)
mark_assignment_dispatched(task_id, dispatched_at)
mark_assignment_failed(task_id, reason)
find_job_detail(job_id)
```

output duplicate 정책:

- [x] 같은 `(job_id, agent_id, stream, chunk_index)`와 같은 body는 idempotent duplicate로 볼 수 있다.
- [x] 같은 key인데 body가 다르면 security/audit 대상이다.
- [x] duplicate 때문에 전체 session이 불필요하게 죽지 않게 한다.

## 테스트와 검증

필수:

- [x] `cargo test -p fleet-domain job`
- [x] `cargo test -p fleet-application dispatch`
- [x] `cargo test -p fleet-store assignment`
- [x] `cargo test -p fleet-store job_output_chunks`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

추가:

- [x] disabled agent pending assignment가 dispatch 대상에서 제외되는 test
- [x] expired assignment가 dispatch되지 않는 test
- [x] DB queued assignment가 send 실패 후 유지되는 test
- [x] duplicate output chunk가 정책대로 처리되는 test

## 완료 기준

- [x] WebSocket handler 없이도 dispatch 정책을 application test로 설명할 수 있다.
- [x] DB queued assignment가 source of truth임이 테스트로 보장된다.
- [x] job status와 assignment/dispatch state의 의미가 문서화된다.
- [x] session registry 구현 전에 domain/application/store 계약이 흔들리지 않는다.

## 비범위

- [x] WebSocket session registry 구현하지 않음
- [x] Agent persistent loop 구현하지 않음
- [x] Web Admin UI 변경하지 않음
