# Task 003: Job/Assignment State Machine Domain Test

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 005
의존성: Task 002
결과물: job과 assignment 상태 모델의 domain 기준선

## 목표

Job과 Assignment를 분리하고, 각 상태 전이를 domain layer에서 테스트로 고정한다. 이 작업은 multi-agent fanout, reconnect, cancel, partial success의 기반이다.

Job은 운영자가 생성하고 추적하는 실행 단위이고, Assignment는 특정 agent에 배정된 실행 단위다. 두 개념을 섞지 않는다.

## 기능 묶음

1. Job state machine 정리
2. Assignment state machine 추가
3. aggregate status 계산 규칙 정의

## 구현 체크리스트

Job 상태:

- [x] 현재 `JobStatus` enum 또는 equivalent model을 찾는다.
- [x] 현재 상태와 저장 값을 조사한다.
- [x] `draft` 필요 여부를 결정한다.
- [x] `pending_approval` 추가 기준을 정한다.
- [x] `queued`, `running`, `success`, `failed`, `canceled`, `expired`의 의미를 문서화한다.
- [x] `partial_success`를 job aggregate 상태로 추가한다.
- [x] 기존 single-agent job과 호환되는 migration 경로를 정한다.

Assignment 상태:

- [x] `AssignmentStatus` enum을 domain layer에 추가한다.
- [x] `queued` 의미를 정의한다.
- [x] `dispatched` 의미를 정의한다.
- [x] `accepted` 의미를 정의한다.
- [x] `started` 의미를 정의한다.
- [x] `output_received`가 상태인지 event인지 결정한다.
- [x] `succeeded`, `failed`, `rejected`, `canceled`, `expired` 의미를 정의한다.
- [x] 허용 transition table을 만든다.
- [x] invalid transition은 domain error로 처리한다.

Aggregate 규칙:

- [x] assignment 전체가 성공하면 job success로 계산한다.
- [x] 일부 성공/일부 실패면 partial_success로 계산한다.
- [x] 전체 실패와 전체 rejected의 차이를 정한다.
- [x] cancel이 일부 target에만 적용된 경우 계산 규칙을 정한다.
- [x] maxFailures 도달 시 job 상태 계산 기준을 정한다.

## 테스트

- [x] Job 생성 초기 상태 테스트
- [x] Job approval 대기 상태 전이 테스트
- [x] Job queued -> running -> success 테스트
- [x] Job queued -> canceled 테스트
- [x] Job running -> partial_success 테스트
- [x] Assignment queued -> dispatched 테스트
- [x] Assignment dispatched -> accepted 테스트
- [x] Assignment accepted -> started 테스트
- [x] Assignment started -> succeeded 테스트
- [x] Assignment started -> failed 테스트
- [x] Assignment dispatched -> rejected 테스트
- [x] invalid transition rejection 테스트
- [x] aggregate status 계산 테스트

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 job과 assignment 상태 의미를 추가한다.
- [x] OpenAPI response model 계획에 job/assignment 구분을 반영한다.
- [x] Web Admin에서 표시할 상태 용어를 README 또는 docs에 정리한다.

## 완료 기준

- [x] Job과 Assignment 상태가 domain test로 고정되어 있다.
- [x] invalid transition이 조용히 무시되지 않는다.
- [x] single-agent run이 multi-agent model의 special case로 설명 가능하다.
- [x] partial_success의 의미가 domain과 문서에서 동일하다.

## 진행 결과

- `fleet-domain`에 `AssignmentStatus`와 `Assignment` domain state machine을 추가했다.
- `Job::mark_partial_success`와 `aggregate_job_status`를 추가해서 multi-agent aggregate 기준선을 만들었다.
- `output_received`는 assignment 상태가 아니라 output storage event로 결정했다.
- 전체 rejected는 job aggregate로 `failed`가 되며, rejected 여부는 assignment/audit에서 구분하도록 정리했다.
- `docs/api.md`와 `docs/openapi.json`에 Job/Assignment 상태 용어와 response model 경계를 반영했다.

## 검증 결과

- [x] `cargo fmt --check`
- [x] `cargo test -p fleet-domain job`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `node -e "JSON.parse(require('fs').readFileSync('docs/openapi.json','utf8'))"`
- [x] `git diff --check`
