# Task 001 - 현재 WebSocket Lifecycle 정리와 테스트 고정

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

현재 Agent-Controller WebSocket이 "상시 연결"이 아니라 heartbeat 주기마다 열고 닫는 구조임을 테스트와 문서로 명확히 고정한다.

이 task는 큰 behavior 변경을 하지 않는다. 이후 persistent session 전환 중 회귀를 확인할 수 있도록 현재 lifecycle, 한계, 전환 목표를 코드 테스트와 문서에 남기는 것이 목적이다.

## 배경

현재 구조:

```text
Agent
  -> controller identity 확인
  -> WebSocket 연결
  -> auth
  -> heartbeat/facts/metrics/log 전송
  -> queued task 1개 수신 가능
  -> output/task_result 전송
  -> 연결 종료
  -> heartbeat interval sleep
```

문제:

- WebSocket을 쓰지만 persistent session이 아니다.
- `Run` 직후 Controller가 Agent에 즉시 task를 push할 수 없다.
- Agent가 sleep 중이면 job은 다음 heartbeat까지 queued 상태로 남는다.
- 이 구조를 모른 채 UI polling만 고치면 근본 문제가 해결되지 않는다.

## 기능 범위

### 1. 현재 lifecycle 테스트 고정

- [x] Controller WebSocket handler가 heartbeat 이후 idle이면 연결을 닫는 현재 동작을 테스트로 기록한다.
- [x] queued assignment가 있을 때 heartbeat 연결에서 task_assignment가 내려가는 현재 동작을 테스트로 기록한다.
- [x] queued assignment가 없으면 즉시 push 경로가 없다는 점을 regression 관점에서 문서화한다.

검토 위치:

- `crates/fleet-controller/src/lib.rs`
- `handle_agent_websocket_axum`
- `read_task_data_until_close_axum`
- `pending_task_assignment_message`

주의:

- 이 task에서 persistent session 구현을 시작하지 않는다.
- 현재 동작 고정 테스트가 너무 구현 세부에 묶이면 이후 refactor를 방해하므로, observable behavior 위주로 작성한다.

### 2. Agent loop 용어 정리 준비

- [x] 현재 `run_agent_heartbeat_loop`, `run_agent_heartbeat_once`, `AgentHeartbeatOptions`의 책임을 정리한다.
- [x] 바로 rename하지 않아도 된다. rename이 필요하면 behavior 변경과 분리된 tidy commit/task로 처리한다.
- [x] CLI help에서 "heartbeat and task loop"가 persistent session 목표와 어떻게 달라질지 메모한다.

검토 위치:

- `crates/fleet-cli/src/lib.rs`
- `sponzey agent start --help`
- `run_agent_heartbeat_loop_with`

### 3. protocol 문서에 현재 한계와 전환 목표 반영

- [x] `docs/protocol.md`에 현재 WebSocket gateway 흐름이 heartbeat-bound임을 명확히 쓴다.
- [x] 같은 문서에 목표 구조가 persistent outbound session임을 쓴다.
- [x] "Controller가 Agent로 직접 접속한다"는 오해가 생기지 않게 표현한다.

## 테스트와 검증

필수:

- [x] `cargo test -p fleet-controller websocket`
- [x] `cargo test -p fleet-cli agent_heartbeat_loop`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

권장:

- [x] 관련 controller unit test 이름에 `heartbeat_bound` 또는 `current_lifecycle`처럼 의도를 드러낸다.
- [x] 테스트가 느려지지 않도록 실제 네트워크 long wait는 피하고 fake/timeout을 짧게 둔다.

## 완료 기준

- [x] 현재 heartbeat-bound WebSocket lifecycle이 테스트로 설명된다.
- [x] persistent session 전환 시 어떤 테스트를 바꿔야 하는지 분명하다.
- [x] 문서가 현재 한계와 목표 구조를 동시에 설명한다.
- [x] behavior 변경 없이 통과한다.

## 비범위

- [x] persistent session registry 구현하지 않음
- [x] Agent loop 구조 변경하지 않음
- [x] job status schema 변경하지 않음
- [x] Web Admin UI 변경하지 않음