# Task 004: Protocol Ack/Start/Reject/Result 분리

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 005
의존성: Task 003
결과물: task dispatch protocol event 분리

## 목표

현재 immediate dispatch를 제품 수준 protocol로 정리한다. WebSocket write 성공을 agent가 task를 수락하거나 실행한 것으로 간주하지 않는다.

Agent는 task를 받았을 때, 실행을 시작했을 때, 거부했을 때, 완료했을 때를 각각 다른 protocol event로 controller에 알려야 한다.

## 기능 묶음

1. protocol message schema 확장
2. controller assignment transition 연결
3. agent task worker event 전송

## 구현 체크리스트

Protocol:

- [x] 현재 task dispatch message schema를 확인한다.
- [x] protocol version 필드를 확인한다.
- [x] message id/correlation id 사용 방식을 확인한다.
- [x] `task_ack` 또는 equivalent event를 설계한다.
- [x] `task_started` event를 설계한다.
- [x] `task_rejected` event를 설계한다.
- [x] `task_output_chunk` event를 final result와 분리한다.
- [x] `task_result` event를 설계한다.
- [x] rejected reason code를 정의한다.
- [x] malformed/unknown message 정책을 정리한다.

Controller:

- [x] dispatch 전 assignment를 store에 기록한다.
- [x] WebSocket write 성공 시 상태를 `dispatched`까지만 전이한다.
- [x] ack 수신 시 `accepted`로 전이한다.
- [x] started 수신 시 `started`로 전이한다.
- [x] rejected 수신 시 `rejected`로 전이한다.
- [x] result 수신 시 terminal state로 전이한다.
- [x] output chunk는 output storage에 저장한다.
- [x] final result 없이는 성공으로 처리하지 않는다.

Agent:

- [x] task 수신 직후 ack를 보낸다.
- [x] signature/expiry/target/capability 검증 실패 시 reject를 보낸다.
- [x] process 실행 직전 started를 보낸다.
- [x] output chunk를 streaming한다.
- [x] process 종료 후 result를 보낸다.
- [x] result 전송 실패 시 reconnect/retry 정책과 충돌하지 않게 처리한다.

## 테스트

- [x] protocol serialization/deserialization test
- [x] unknown field compatibility test
- [x] controller dispatch writes assignment before send test
- [x] ack updates assignment to accepted test
- [x] started updates assignment to started test
- [x] rejected updates assignment to rejected test
- [x] output chunk does not mark success test
- [x] final result marks terminal state test
- [x] invalid signature causes rejected test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 assignment event와 result 의미를 정리한다.
- [x] protocol 관련 docs가 있으면 ack/start/reject/result를 반영한다.
- [x] Web Admin job output 설명에서 output과 result를 구분한다.

## 완료 기준

- [x] WebSocket write 성공이 accepted/started/success와 혼동되지 않는다.
- [x] controller는 agent의 명시 event를 통해 assignment 상태를 갱신한다.
- [x] output chunk와 final result가 분리되어 저장된다.
- [x] protocol compatibility test가 있다.

## 진행 결과

- `fleet-protocol`에 `task_ack`, `task_started`, `task_rejected` payload와 rejected reason code를 추가했다.
- `task_assignments` schema version을 4로 올리고 `status`, `dispatched_at`, `accepted_at`, `started_at`, `completed_at`, `last_error`를 추가했다.
- dispatch 성공 시 assignment는 `dispatched`로 저장하고, ack/start/reject/result 수신 시 `accepted`, `started`, `rejected`, `succeeded/failed`로 갱신한다.
- Agent worker는 검증 성공 후 ack, 실행 직전 started, 검증 실패 또는 busy 상태에서 rejected를 보낸다.
- `output_chunk`는 output storage에만 저장하고 final success/failure는 `task_result`로만 처리한다.
- `docs/api.md`, `docs/protocol.md`, `docs/storage.md`, `docs/feature-matrix.md`, `docs/release-notes-mvp.md`를 현재 구현 기준으로 업데이트했다.

## 검증 결과

- [x] `cargo fmt --check`
- [x] `cargo test -p fleet-protocol -p fleet-application -p fleet-store -p fleet-controller -p fleet-cli task`
- [x] `cargo test -p fleet-controller dispatch`
- [x] `cargo test -p fleet-cli agent_session`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `git diff --check`
