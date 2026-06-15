# Task 009 - Revoke/Offline/Session Close 정책

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

persistent session에서 보안 이벤트와 상태 변화가 즉시 반영되도록 revoke, offline, session close 정책을 구현한다.

특히 Agent key revoke는 "다음 heartbeat 때 차단"이 아니라 active session close까지 포함해야 한다.

## 기능 범위

### 1. Revoke 즉시 active session close

- [x] revoke agent key API 성공 직후 session registry에서 해당 agent session을 close한다.
- [x] close reason은 `agent_revoked`로 남긴다.
- [x] revoked agent에는 새 task dispatch가 막힌다.

정책:

- queued assignment는 더 이상 dispatch하지 않는다.
- running task는 이미 OS process로 실행 중일 수 있으므로 즉시 kill 보장은 하지 않는다.
- task cancellation protocol 전까지 timeout을 running process boundary로 사용한다.

문서화:

- [x] revoke는 추가 task 수신 차단과 session 종료를 보장한다고 설명한다.
- [x] 이미 실행 중인 local process kill은 별도 cancellation 기능이라고 설명한다.

### 2. Offline 판정 재정의

- [x] active authenticated session이 있으면 online으로 본다.
- [x] session은 없지만 last_seen_at이 threshold 이내면 reconnecting 또는 recently_seen으로 볼 수 있다.
- [x] threshold 초과는 offline이다.
- [x] revoked/disabled는 offline + revoked로 표시한다.

필요 설정 후보:

- `--agent-session-idle-timeout-seconds`
- `--agent-heartbeat-timeout-seconds`

규칙:

- 설정은 bootstrap에서만 받는다.
- UI/API로 runtime 변경하지 않는다.

### 3. Session close reason audit/log

- [x] `agent_session_started`
- [x] `agent_session_ended`
- [x] `agent_session_replaced`
- [x] `agent_session_revoked_closed`
- [x] `agent_session_auth_failed`

로그 정책:

- Product log는 상태 변화 중심으로 낮은 볼륨 유지.
- Field debug에는 connection id, close reason, duration, queue depth 포함.
- token/private key/raw output은 기록하지 않는다.

## 테스트와 검증

필수:

- [x] revoke API가 active session을 제거하는 test
- [x] revoked agent reconnect 거부 test
- [x] revoke 직후 새 task dispatch가 막히는 test
- [x] active session close 후 agent inventory offline+revoked 표시 test
- [x] duplicate session replacement audit test
- [x] heartbeat timeout/offline transition test
- [x] `cargo test -p fleet-controller revoke`
- [x] `cargo test -p fleet-domain agent`
- [x] `npm test --workspace web-admin`
- [x] `git diff --check`

## 완료 기준

- [x] revoked agent는 active session에서 제거된다.
- [x] revoked agent는 더 이상 task를 받을 수 없다.
- [x] UI/API는 offline과 revoked를 함께 보여줄 수 있다.
- [x] session close reason이 audit/log로 남는다.
- [x] running process kill 한계가 문서화된다.

## 비범위

- [x] task cancellation protocol 구현하지 않음
- [x] running OS process 즉시 kill 보장하지 않음
- [x] multi-controller HA session migration 구현하지 않음