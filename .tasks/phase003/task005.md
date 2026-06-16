# Task 005 - Agent Persistent Session Loop

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

Agent가 heartbeat마다 연결을 열고 닫는 구조에서 벗어나, Controller와 WebSocket session을 유지하도록 Agent loop를 전환한다.

이 task의 핵심은 Agent 내부에서도 single writer queue를 두고, task 실행 중에도 heartbeat/liveness와 session read/write가 막히지 않게 하는 것이다.

## 기능 범위

### 1. Agent session loop 도입

- [x] `run_agent_session_loop` 또는 이에 준하는 persistent session loop를 구현한다.
- [x] 기존 `run_agent_heartbeat_loop`와 CLI 호환성을 유지한다.
- [x] `--heartbeat-interval-seconds`는 연결 주기가 아니라 liveness tick interval로 의미를 재정의한다.

동작:

```text
agent start
  -> load config
  -> controller identity pinning 확인
  -> websocket connect
  -> agent_hello/auth/auth_accepted
  -> persistent read/write loop
  -> failure 시 reconnect backoff
```

체크:

- [x] `--once`는 smoke/test 용도로 한 번 연결/처리 후 종료할 수 있어야 한다.
- [x] network failure는 기본적으로 종료하지 않고 retry한다.
- [x] controller fingerprint mismatch는 fatal로 유지한다.

### 2. Single outbound writer queue

- [x] Agent 내부 WebSocket writer는 하나만 둔다.
- [x] heartbeat, facts, metrics, log, output producer는 outbound queue에 message를 넣는다.
- [x] command output callback이 socket에 직접 쓰지 않게 바꾼다.

필수:

- [x] task worker가 오래 실행되어도 heartbeat가 전송된다.
- [x] outbound queue가 가득 찰 때 무제한 메모리 증가를 막는다.
- [x] output limit 초과와 write failure를 task/session failure로 명확히 전환한다.

### 3. Task worker 분리

- [x] read loop는 `task_assignment`를 받고 task worker에 work item을 넘긴다.
- [x] task worker는 signature/expiry/nonce/target 검증 후 실행한다.
- [x] output_chunk와 task_result는 outbound queue를 통해 전송한다.

초기 concurrency 정책:

- [x] Agent당 전체 task concurrency는 1로 둔다.
- [x] busy 상태에서 새 task를 받으면 reject 또는 queued 정책을 명확히 한다.
- [x] low-risk drift와 high-risk command 동시 실행은 후속 정책으로 미룬다.

## 테스트와 검증

필수:

- [x] session loop reconnect test
- [x] heartbeat interval test
- [x] controller close 후 retry test
- [x] 긴 task 실행 중 heartbeat가 막히지 않는 test
- [x] task worker가 socket writer에 직접 쓰지 않는 구조 검토
- [x] `cargo test -p fleet-cli agent_session`
- [x] `cargo test -p fleet-runner streaming`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

권장:

- [x] outbound queue full fault injection test
- [ ] revoked/auth rejected 시 retry 정책 test
- [x] controller fingerprint mismatch fatal test 유지

## 완료 기준

- [x] Agent는 정상 상태에서 Controller와 WebSocket을 유지한다.
- [x] connection failure 시 기본적으로 종료하지 않고 retry한다.
- [x] task 실행 중 heartbeat/liveness가 막히지 않는다.
- [x] Agent socket writer는 단일 queue 기반이다.
- [x] 기존 enrollment, pinning, signature 검증이 유지된다.

## 비범위

- [x] Controller job 생성 즉시 dispatch 연결하지 않음
- [x] Web Admin UI 변경하지 않음
- [x] task cancellation protocol 구현하지 않음
