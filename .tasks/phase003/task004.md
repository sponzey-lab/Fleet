# Task 004 - Controller WebSocket Read/Write Loop 분리

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

Controller의 Agent WebSocket handler를 read loop와 write loop로 분리하여, 인증된 session으로 나중에 task를 push할 수 있는 구조를 만든다.

현재처럼 함수 하나가 순차적으로 read/write를 처리하면 `Run` 요청이 들어온 시점에 이미 연결된 Agent에게 push할 writer 경로가 없다. 이 task는 persistent session의 Controller 쪽 기반이다.

## 기능 범위

### 1. WebSocket split과 단일 writer loop

- [x] Agent WebSocket 인증 후 socket을 read half/write half로 분리한다.
- [x] write half는 session당 하나의 writer loop만 소유한다.
- [x] 다른 producer는 outbound channel로 `WireMessage`를 전달한다.

구조:

```text
handle_agent_websocket
  -> authenticate
  -> create bounded outbound channel
  -> register session
  -> read loop
  -> write loop
  -> cleanup
```

규칙:

- [x] 여러 async task가 같은 WebSocket writer에 직접 쓰지 않는다.
- [x] outbound channel size는 제한한다.
- [x] channel overflow는 session failure 또는 dispatch failure로 처리한다.
- [x] write failure는 session cleanup으로 이어진다.

### 2. Read loop 정리

- [x] read loop는 authenticated Agent message만 처리한다.
- [x] heartbeat/facts/metrics/log/output/task_result/drift/security_event의 agent_id를 session agent_id와 비교한다.
- [x] mismatch는 저장하지 않고 security audit 대상으로 처리한다.

주의:

- store lock을 잡은 상태에서 `.await`하지 않는다.
- DB write 실패가 session 전체를 죽일지, message만 실패 처리할지 정책을 문서화한다.
- raw command output은 Product log로 흘리지 않는다.

### 3. Session cleanup과 close reason

- [x] read loop 또는 write loop 어느 한쪽이 종료되면 registry에서 session을 제거한다.
- [x] close reason을 남긴다.
- [x] duplicate replacement, revoke, heartbeat timeout, protocol error를 구분할 수 있게 한다.

close reason 후보:

- `normal_shutdown`
- `idle_timeout`
- `heartbeat_timeout`
- `replaced_by_new_session`
- `agent_revoked`
- `auth_failed`
- `protocol_error`
- `write_queue_overflow`
- `store_error`

## 테스트와 검증

필수:

- [x] outbound channel로 task_assignment를 writer loop에 전달하는 test
- [x] write failure 시 session cleanup test
- [x] channel overflow 시 queued 유지 또는 dispatch failure audit test
- [x] agent_id mismatch payload가 security audit으로 처리되는 test
- [x] `cargo test -p fleet-controller websocket`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

코드 리뷰 체크:

- [x] store lock 범위와 `.await`가 겹치지 않는다.
- [x] WebSocket writer는 session당 하나다.
- [x] output body 원문이 tracing info/warn/error에 들어가지 않는다.

## 완료 기준

- [x] Agent가 idle 상태로 연결을 유지할 수 있는 Controller 구조가 생겼다.
- [x] Controller 내부에서 active session에 message push가 가능하다.
- [x] session 종료/교체/오류가 registry cleanup과 연결된다.
- [x] WebSocket handler가 dispatch business rule을 직접 크게 갖지 않는다.

## 비범위

- [x] Agent persistent loop 완성하지 않음
- [x] Web Admin 상태 UX 변경하지 않음
- [x] SSE/Admin streaming output 만들지 않음
