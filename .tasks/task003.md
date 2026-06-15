# Task 003 - Controller Active Session Registry

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

Controller가 인증된 Agent WebSocket session을 runtime state로 추적할 수 있게 한다.

이 task는 "Agent가 연결되어 있는가", "해당 Agent에게 보낼 outbound channel이 있는가", "duplicate session을 어떻게 처리하는가"를 다룬다. 실제 task dispatch push는 다음 task들에서 연결한다.

## 배경

persistent session에서 Controller는 Agent로 직접 접속하지 않는다. Agent가 Controller에 outbound WebSocket을 열고, Controller는 그 session writer를 통해 task를 보낸다.

따라서 Controller process에는 active session registry가 필요하다.

중요 원칙:

- session registry는 runtime infrastructure state다.
- WebSocket handle이나 channel sender를 DB에 저장하지 않는다.
- Domain object에 WebSocket type, tokio channel, socket split type을 넣지 않는다.
- Controller restart 시 registry는 비어지고, Agent reconnect로 회복한다.

## 기능 범위

### 1. AgentSessionRegistry 추가

- [x] `AgentSessionRegistry` 구조체 또는 trait을 추가한다.
- [x] agent id 기준으로 active session handle을 등록/조회/제거할 수 있게 한다.
- [x] session handle에는 WebSocket 자체가 아니라 outbound sender와 metadata만 둔다.

권장 필드:

```text
AgentSessionHandle
  agent_id
  connection_id
  connected_at
  last_seen_at
  capabilities
  outbound_sender
  queue_depth 또는 queue_capacity
```

필수 API 후보:

```text
register(agent_id, connection_id, capabilities, outbound_sender)
unregister(agent_id, connection_id, close_reason)
replace(agent_id, new_session)
get(agent_id)
close(agent_id, reason)
snapshot()
```

체크:

- [x] registry operation은 빠르게 끝난다.
- [x] store lock과 registry lock을 중첩해서 오래 잡지 않는다.
- [x] snapshot은 Web Admin/API 표시용 DTO로 변환 가능하다.

### 2. Duplicate session 정책 구현

- [x] 같은 agent id로 새 session이 인증되면 new session wins 정책을 적용한다.
- [x] 기존 session에는 close reason `replaced_by_new_session`을 전달한다.
- [x] replacement가 audit/log에 남을 수 있게 이벤트 정보를 제공한다.

정책:

- 기존 session이 늦게 닫히더라도 새 session은 등록되어야 한다.
- old session에서 뒤늦게 message가 오면 connection_id mismatch로 무시하거나 정리한다.
- 악의적 duplicate는 auth 단계에서 identity proof를 거치므로 fingerprint/public key 검증을 계속 유지한다.

### 3. Session summary DTO

- [x] `/api/agents` 또는 신규 session API에서 사용할 summary shape를 준비한다.
- [x] DB 상태와 runtime state를 분리해서 표현한다.
- [x] UI가 connected 여부를 추측하지 않도록 API에 전달할 수 있는 구조를 만든다.

필드 후보:

```json
{
  "agent_id": "agent-1",
  "connected": true,
  "connected_at_ms": 1710000000000,
  "last_session_seen_at_ms": 1710000005000,
  "capabilities": ["persistent_session"]
}
```

## 테스트와 검증

필수:

- [x] session 등록/조회/해제 unit test
- [x] duplicate session replacement test
- [x] unregister 시 connection_id가 다르면 새 session을 지우지 않는 test
- [x] revoked agent가 registry에 남지 않게 close/remove 가능한 test
- [x] `cargo test -p fleet-controller session`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

권장:

- [x] registry snapshot ordering이 deterministic한 test
- [x] outbound queue full 상태를 summary에 표현할 수 있는 test

## 완료 기준

- [x] Controller가 active agent session 여부를 조회할 수 있다.
- [x] session registry는 DB가 아니라 runtime state다.
- [x] duplicate session 정책이 테스트로 고정된다.
- [x] session summary를 API/UI에 연결할 준비가 됐다.

## 비범위

- [x] WebSocket split/write loop 구현하지 않음
- [x] job 생성 후 즉시 dispatch 연결하지 않음
- [x] Agent persistent loop 구현하지 않음
