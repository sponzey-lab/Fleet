# Sponzey Fleet 상시 Agent 연결 전환 계획

상위 목표: `Run`을 누른 뒤 다음 heartbeat까지 기다리는 구조를 제거하고, Agent가 Controller에 열어 둔 WebSocket 연결을 통해 task를 즉시 받도록 전환한다.

현재 기준일: 2026-06-15

현재 상태:

- Controller와 Agent는 WebSocket을 사용한다.
- 하지만 WebSocket은 상시 연결이 아니라 heartbeat 주기마다 열고, heartbeat/facts/metrics/log를 보낸 뒤, queued task가 있으면 하나를 받고, 결과를 보낸 뒤 닫히는 왕복형 연결에 가깝다.
- 따라서 Web Admin의 `Run` 결과는 command 실행 시간이 아니라 Agent의 다음 heartbeat 시점에 지연된다.
- 기본 heartbeat가 30초라면 사용자는 정상 상황에서도 최대 30초 가까이 `No job output` 또는 `Waiting` 상태를 볼 수 있다.
- 현재 `jobs.status`의 domain 상태는 `queued`, `running`, `success`, `failed`, `canceled`, `expired` 등을 중심으로 되어 있고 `dispatched`, `accepted`, `rejected`는 아직 없다.
- 현재 `task_assignments` 테이블은 이미 존재하지만 assignment 자체의 status, dispatched_at, started_at, completed_at, last_error는 없다.
- 현재 `job_output_chunks`는 `(job_id, agent_id, stream, chunk_index)` unique constraint가 있어 output chunk 중복 방어의 기반은 있다.

목표 상태:

- Agent는 Controller에 outbound WebSocket을 상시 유지한다.
- Controller는 Agent로 직접 TCP 연결하지 않는다.
- `Run` 요청이 들어오면 Controller는 이미 인증된 Agent WebSocket session으로 task를 즉시 push한다.
- Agent는 task를 즉시 검증하고 실행한다.
- output chunk와 task result는 같은 Agent WebSocket session으로 즉시 Controller에 전송된다.
- heartbeat는 "연결을 여는 계기"가 아니라 "상시 session의 liveness signal"이 된다.
- facts, metrics, operational log 전송 주기는 heartbeat와 분리한다.

## 0. 깊은 검토 후 보완한 판단

이 계획은 단순히 WebSocket 연결 시간을 늘리는 작업이 아니다. 실제 제품 방향에 맞게 다음 판단을 기준으로 보완한다.

- Controller가 Agent로 접속하는 inbound 모델은 채택하지 않는다. NAT, 방화벽, 노트북/서버 위치 차이를 고려하면 Agent outbound persistent session이 제품 방향이다.
- 현재 WebSocket은 이미 있지만 persistent session이 아니다. "WebSocket이 있으니 즉시 push된다"는 전제는 틀렸고, session registry와 write loop가 있어야 즉시 push가 가능하다.
- 즉시 push는 DB보다 우선할 수 없다. job과 assignment가 먼저 저장되고, 그 다음 active session으로 dispatch되어야 한다.
- DB transaction과 WebSocket send는 하나의 원자적 transaction이 될 수 없다. 따라서 queued assignment를 source of truth로 두고 idempotent dispatch/recovery 정책을 명확히 해야 한다.
- 상시 연결에서는 WebSocket writer가 하나여야 한다. heartbeat loop, task output worker, facts/metrics loop가 socket에 직접 동시에 쓰면 안 된다.
- command/runbook 실행은 WebSocket read loop와 heartbeat/liveness를 막으면 안 된다. 긴 command 실행 중에도 session은 살아 있어야 한다.
- Agent key revoke는 "다음 heartbeat 때 반영"이 아니라 active session close까지 포함해야 한다.
- UI는 domain rule을 재구현하지 않는다. job/session 상태를 API로 받고 상태별 문구만 표시한다.
- HTTP/WS 허용 정책은 바꾸지 않는다. 기술적으로 허용해도 테스트 전용 경고와 audit는 유지한다.

## 1. 문제 정의

### 1.1 현재 WebSocket 구조의 한계

현재 구조는 다음 흐름이다.

```text
Agent loop
  -> Controller identity 확인
  -> WebSocket 연결
  -> agent_hello
  -> auth_challenge/auth_response/auth_accepted
  -> heartbeat
  -> facts snapshot
  -> metrics snapshot
  -> optional log chunk
  -> controller가 queued task 1개 있으면 task_assignment 수신
  -> task 실행
  -> output_chunk/task_result 전송
  -> 연결 종료
  -> heartbeat interval sleep
```

이 구조의 문제:

- `Run`은 Controller DB에 job을 저장할 뿐 Agent에게 즉시 전달하지 못한다.
- Agent가 연결을 닫고 sleep 중이면 Controller는 전달 경로가 없다.
- WebSocket을 쓰고 있음에도 사용자 경험은 polling agent와 유사하다.
- heartbeat interval을 줄이면 반응성은 좋아지지만 연결/인증/facts/metrics/log 비용이 같이 증가한다.
- facts/metrics 수집 주기와 task dispatch 반응성이 서로 묶여 있어 운영 정책을 세밀하게 조정하기 어렵다.
- Web Admin은 "아직 agent가 job을 받지 않음", "agent가 실행 중", "실행 완료지만 output 없음", "agent offline"을 구분하기 어렵다.

### 1.2 바뀌어야 하는 핵심 정책

상시 연결로 전환하면 다음 정책이 바뀐다.

- heartbeat 정책
  - 기존: 연결 주기이자 online 표시의 트리거
  - 변경: 이미 열린 session의 liveness tick
- online/offline 정책
  - 기존: 최근 heartbeat 저장 시각 기준
  - 변경: active authenticated session 우선, 보조로 last heartbeat age 사용
- task dispatch 정책
  - 기존: Agent heartbeat 요청 중 pending job 1개를 내려줌
  - 변경: job 생성 또는 assignment 생성 시 active session으로 즉시 push
- task queue 정책
  - 기존: DB pending assignment만 있으면 다음 heartbeat에서 처리
  - 변경: connected agent에는 즉시 dispatch, disconnected agent에는 DB queued 유지
- revoke 정책
  - 기존: 다음 연결/heartbeat 때 비활성 agent가 차단됨
  - 변경: Agent key revoke 즉시 active session을 끊고, 이후 task dispatch도 중단
- facts/metrics/log 정책
  - 기존: heartbeat cycle마다 함께 전송
  - 변경: 각자 독립 interval로 상시 session 안에서 전송
- output 정책
  - 기존: task를 받은 연결에서만 output을 보내고 연결 종료
  - 변경: long-lived session에서 streaming output chunk를 계속 수신하고 저장
- failure/retry 정책
  - 기존: heartbeat_once 실패 후 reconnect backoff
  - 변경: persistent session read/write loop 실패 시 즉시 session 정리 후 reconnect backoff

## 2. 아키텍처 원칙

AGENTS.md의 원칙을 유지한다.

- 제품 바이너리는 계속 `sponzey` 하나다.
- Controller와 Agent는 별도 binary가 아니라 subcommand 역할이다.
- Controller가 Agent로 직접 접속하지 않는다. Agent outbound 연결 모델을 유지한다.
- Domain layer는 WebSocket, tokio, SQLite 세부사항을 알지 않는다.
- Application layer는 "task를 어느 agent에 dispatch할지"와 "job 상태를 어떻게 바꿀지"를 다룬다.
- Infrastructure/interface layer가 WebSocket session, network IO, storage 구현을 담당한다.
- 설정은 bootstrap 시점에만 읽고 immutable settings로 전달한다.
- runtime 중 env var 변경이나 UI/API를 통한 process env patch는 금지한다.
- task signature, agent identity proof, controller identity pinning은 유지한다.
- HTTP transport는 기술적으로 가능하더라도 테스트 전용 경고 정책을 유지한다.

구현 금지:

- `SqliteStore` lock을 잡은 상태에서 WebSocket send/read 같은 `.await`를 수행하지 않는다.
- HTTP handler나 WebSocket handler 안에 job dispatch business rule을 직접 크게 작성하지 않는다. 얇은 interface layer에서 application service로 위임한다.
- Domain layer에 WebSocket session, tokio channel, socket split type, database connection을 넣지 않는다.
- Agent task worker가 WebSocket writer를 직접 여러 곳에서 공유해서 동시에 쓰지 않는다.
- 즉시성을 위해 unsigned task, signature 검증 생략, expiry 검증 생략, nonce replay guard 생략을 허용하지 않는다.
- runtime API나 Web Admin UI로 heartbeat/facts/metrics interval을 바꾸지 않는다. 이 값들은 process bootstrap 설정이다.

권장 경계:

```text
HTTP/WebSocket Interface
  -> SessionRegistry interface
  -> Application dispatch use case
  -> Store/repository contracts
  -> Domain job/task/session policy
```

Session registry는 runtime infrastructure다. Domain에는 "agent가 connected인지", "dispatch 가능한 상태인지" 같은 정책 판단에 필요한 값만 DTO 또는 application input으로 전달한다.

## 3. 목표 동작

### 3.1 Agent start

```text
sponzey agent start
  -> local agent config 로드
  -> controller identity 확인 및 pinning 검증
  -> WebSocket 연결
  -> agent_hello
  -> auth challenge-response
  -> auth_accepted
  -> session_ready 또는 heartbeat 전송
  -> read loop와 periodic loop 시작
```

Agent process는 종료되지 않고 다음 일을 병렬로 수행한다.

- Controller에서 task_assignment 수신
- task signature/expiry/nonce/target 검증
- command/runbook/drift task 실행
- output_chunk streaming
- task_result 전송
- heartbeat/liveness message 주기 전송
- facts snapshot 주기 전송
- metrics snapshot 주기 전송
- operational log chunk 주기 전송
- connection failure 시 reconnect backoff

### 3.2 Web Admin Run

```text
Web Admin Run
  -> POST /api/jobs/command
  -> Controller create job + assignment
  -> Controller active session registry 확인
  -> Agent가 connected이면 즉시 task_assignment push
  -> Agent가 disconnected이면 queued 상태 유지
  -> Web Admin은 job status/output을 갱신
```

연결된 Agent 기준 기대 UX:

- `Run` 클릭 후 1초 이내 job status가 `dispatching` 또는 `running`으로 변한다.
- command가 빠르게 끝나면 output이 거의 즉시 보인다.
- Agent가 offline이면 "queued until agent reconnects"처럼 명확히 표시한다.

### 3.3 Offline Agent

Agent가 disconnected이면 Controller는 Agent로 직접 접속하지 않는다.

정책:

- job assignment는 DB에 queued로 남긴다.
- Agent가 다시 연결되고 인증되면 queued assignment를 즉시 dispatch한다.
- job expiry가 지나면 expired 처리한다.
- Web Admin에는 `queued`, `agent offline`, `expires at`을 보여준다.

## 4. Protocol 변경 계획

### 4.1 기존 payload 유지

다음 payload는 유지한다.

- `agent_hello`
- `auth_challenge`
- `auth_response`
- `auth_accepted`
- `heartbeat`
- `task_assignment`
- `output_chunk`
- `task_result`
- `facts_snapshot`
- `metrics_snapshot`
- `log_chunk`
- `security_event`
- `drift_report`

### 4.2 추가를 검토할 payload

상시 연결 안정화를 위해 다음 payload를 추가한다.

- `session_ready`
  - Agent가 인증 완료 후 task 수신 준비가 되었음을 명시한다.
  - 기존 heartbeat 첫 메시지로 대체할 수도 있지만, heartbeat와 session lifecycle을 분리하는 편이 명확하다.
- `task_ack`
  - Agent가 task_assignment를 수신하고 signature 검증 전 또는 검증 후 접수했음을 알린다.
  - Controller는 `queued -> dispatched` 전이를 명확히 할 수 있다.
- `task_started`
  - Agent가 검증을 통과하고 실제 실행을 시작했음을 알린다.
  - Controller는 `dispatched -> running` 전이를 명확히 할 수 있다.
- `task_rejected`
  - Agent가 signature, expiry, target mismatch, replay, local policy 위반으로 task를 거부했음을 알린다.
  - 기존 `security_event`로도 표현 가능하지만, job 상태 전이를 위해 별도 payload가 더 명확할 수 있다.
- `ping` / `pong`
  - WebSocket protocol-level ping/pong으로 충분하면 별도 payload는 만들지 않는다.
  - application-level heartbeat만 필요하면 기존 `heartbeat`를 계속 사용한다.

### 4.3 Protocol version 정책

상시 연결 전환은 wire behavior가 바뀌지만 기존 payload를 유지하면서 진행할 수 있다.

정책:

- `protocol_version`은 당장 `1`을 유지할 수 있다.
- 새로운 payload를 추가하면 old agent는 unknown message type을 reject할 수 있으므로 compatibility 검토가 필요하다.
- 한 번에 깨는 대신 단계적으로 적용한다.

단계:

1. 기존 `heartbeat` 이후 연결을 닫지 않고 유지한다.
2. 기존 `task_assignment`, `output_chunk`, `task_result`만으로 즉시 dispatch를 구현한다.
3. job status 정밀화가 필요할 때 `task_ack`, `task_started`, `task_rejected`를 추가한다.
4. 새 payload를 추가하면 `protocol_version` 또는 capability negotiation을 도입한다.

### 4.4 Capability negotiation

상시 연결 agent와 기존 heartbeat agent를 구분하려면 capability가 필요하다.

후보:

```json
{
  "type": "agent_hello",
  "payload": {
    "agent_id": "agent-1",
    "fingerprint": "...",
    "capabilities": ["persistent_session", "streaming_output_v1"]
  }
}
```

현재 `agent_hello` shape를 바꾸면 compatibility 영향이 있다. Serde 기본값 처리로 optional field를 추가하거나, 별도 `session_ready` payload에서 capability를 전달한다.

권장:

- `AgentHello`에 optional `capabilities: Vec<String>`를 추가하되 `#[serde(default)]`로 기존 message를 수용한다.
- Controller는 capability가 없으면 기존 heartbeat 방식 fallback을 유지할 수 있다.
- 제품 정책상 빠르게 전환하려면 fallback 기간을 짧게 잡고 문서화한다.

### 4.5 1차 전환의 protocol 원칙

1차 구현은 protocol surface를 최소화한다.

- 기존 `agent_hello`, `auth_challenge`, `auth_response`, `auth_accepted`, `heartbeat`, `task_assignment`, `output_chunk`, `task_result`를 그대로 사용한다.
- Controller는 인증 후 연결을 닫지 않고 session registry에 writer handle을 등록한다.
- Agent는 인증 후 연결을 유지하며 `task_assignment`를 계속 기다린다.
- Agent는 주기적으로 기존 `heartbeat` payload를 보내 liveness를 알린다.
- Controller는 `heartbeat`를 "새 연결 요청"이 아니라 "이미 인증된 session의 liveness event"로 처리한다.
- task 상태 정밀화를 위한 `task_ack`, `task_started`, `task_rejected`는 2차 구현에서 추가한다.

이유:

- 새 wire payload를 한 번에 늘리면 기존 agent/client compatibility를 판단하기 어렵다.
- 즉시 dispatch 문제는 payload 추가보다 session lifecycle 문제다.
- 최소 변경으로 "Run 즉시 전달"을 먼저 검증한 뒤 상태 정밀화를 추가한다.

### 4.6 Session close reason 정책

WebSocket close는 운영자가 원인을 추적할 수 있어야 한다.

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

정책:

- Product log에는 상태 변화 중심으로 남긴다.
- Security 성격의 close는 audit에도 남긴다.
- Field debug에는 connection id, agent id, reason, connected duration, queue depth를 남긴다.

## 5. Controller 변경 계획

### 5.1 Active Session Registry

Controller process 안에 active session registry를 둔다.

역할:

- authenticated agent session을 agent id 기준으로 추적한다.
- job 생성 시 해당 agent session에 task를 즉시 push한다.
- revoke/offline/shutdown 시 session을 닫는다.
- duplicate session 정책을 적용한다.

중요 원칙:

- registry는 runtime ephemeral state다.
- DB에 WebSocket handle을 저장하지 않는다.
- Controller restart 시 모든 session은 사라지고 Agent가 reconnect한다.
- registry는 infrastructure/interface layer에 두고 domain object로 오염시키지 않는다.

권장 구조:

```text
ControllerAppState
  store: Arc<Mutex<SqliteStore>>
  sessions: Arc<AgentSessionRegistry>

AgentSessionRegistry
  map: agent_id -> AgentSessionHandle

AgentSessionHandle
  agent_id
  connection_id
  connected_at
  last_seen_at
  capabilities
  outbound_sender
  queue_depth
```

registry API 후보:

```text
register(agent_id, connection_id, capabilities, outbound_sender)
unregister(agent_id, connection_id, close_reason)
replace(agent_id, new_session)
get(agent_id) -> Option<AgentSessionHandle>
close(agent_id, reason)
snapshot() -> Vec<AgentSessionSummary>
```

주의:

- `AgentSessionHandle`에는 WebSocket 자체를 넣지 않고 bounded outbound channel sender만 둔다.
- registry operation은 빠르게 끝나야 하며 DB lock과 섞지 않는다.
- session summary는 Web Admin/API 표시용 DTO로만 노출한다.

### 5.2 Writer/Reader 분리

현재 handler는 socket을 함수 하나에서 순차적으로 읽고 쓴다.

상시 연결에서는 다음 구조가 필요하다.

```text
WebSocket split
  read loop
    <- heartbeat/facts/metrics/log/output/task_result
  write loop
    -> task_assignment/control messages
  internal mpsc channel
    Controller job API -> session registry -> session writer
```

정책:

- write loop가 막히면 해당 agent session을 unhealthy로 보고 종료한다.
- per-agent outbound channel size를 제한한다.
- channel overflow는 task를 유실하지 않고 DB queued 상태로 되돌리거나 dispatch 실패로 기록한다.
- output 수신은 storage write 실패 시 task/session 정책을 명확히 한다.
- socket write는 write loop 한 곳에서만 수행한다.
- heartbeat, facts, metrics, task output 같은 여러 생산자는 outbound channel에 message를 넣을 뿐 socket에 직접 쓰지 않는다.
- read loop는 controller가 받는 message를 store/application으로 넘기되, store lock을 잡고 await하지 않는다.

권장 구조:

```text
handle_agent_websocket
  -> authenticate
  -> split socket
  -> create bounded outbound channel
  -> register session
  -> spawn/read loop
  -> spawn/write loop
  -> drain pending queue
  -> cleanup on either loop end
```

### 5.3 Duplicate Session 정책

같은 agent id로 두 개의 session이 생길 수 있다.

원인:

- 네트워크 단절 후 old TCP가 늦게 닫힘
- agent process 중복 실행
- 악의적 재접속 시도

정책 후보:

1. New session wins
   - 새 인증 session이 오면 기존 session을 닫는다.
   - 운영상 가장 단순하다.
2. Old session wins
   - 이미 연결된 session이 있으면 새 session을 거부한다.
   - agent restart 중 복구가 느려질 수 있다.

권장:

- New session wins.
- 기존 session close reason: `replaced_by_new_session`.
- audit: `agent_session_replaced`.
- Product log: 낮은 볼륨으로 기록.
- Field debug: connection id, previous age, capabilities 기록.

### 5.4 Job Dispatch 상태 전이

현재 job status는 대략 `queued`, `running`, `success`, `failed`, `expired` 중심이다.

상시 연결에서는 다음 상태가 필요하다.

```text
queued
  -> dispatching
  -> dispatched
  -> running
  -> success
  -> failed
  -> expired
  -> canceled
  -> rejected
```

최소 구현에서는 상태 폭발을 피하기 위해 다음부터 시작한다.

- `queued`: DB에 있고 아직 Agent에게 전달되지 않음
- `running`: Agent session으로 task_assignment를 보냈고 실행 중으로 간주
- `success` / `failed`: task_result 수신
- `expired`: expires_at 초과
- `rejected`: Agent가 보안/정책 이유로 거부

후속으로 `dispatched`와 `accepted`를 추가한다.

중요 정책:

- Controller가 active session에 `task_assignment`를 send 성공하면 최소 `running` 또는 `dispatched`로 바꾼다.
- send 실패 시 job은 `queued`로 남기거나 재시도 대상으로 둔다.
- Agent가 task_result를 보내지 않고 연결이 끊기면 job은 즉시 failed로 만들지 않는다.
- running job의 timeout/expiry watcher가 필요하다.

### 5.5 Immediate Dispatch Trigger

job 생성 API 이후 즉시 dispatch를 시도한다.

대상:

- `POST /api/jobs/command`
- `POST /api/jobs/runbook`
- `POST /api/jobs/drift-check`
- future approval approve endpoint

흐름:

```text
create job use case
  -> save assignment
  -> commit
  -> dispatch service try_dispatch(job_id or agent_id)
  -> active session 있으면 task_assignment push
  -> 없으면 queued 유지
```

주의:

- DB 저장 전 WebSocket으로 먼저 보내면 안 된다.
- send 성공 후 DB status update 실패 시 불일치가 생길 수 있다.
- 최소 구현에서는 DB 저장 후 send, send 성공 후 status update 순서를 사용하고 실패를 audit한다.
- 더 엄격한 구현은 outbox pattern을 사용한다.

필수 application service:

```text
DispatchPendingAssignments
  input: agent_id 또는 job_id
  reads: pending assignments
  checks: agent status, assignment expiry, active session availability
  sends: task_assignment through SessionDispatcher
  writes: job/assignment dispatch state
  audits: dispatch success/failure
```

dispatch trigger는 두 곳이다.

- job 생성 직후: connected agent에 즉시 push
- agent session 등록 직후: disconnected 동안 쌓인 pending assignment drain

이 두 trigger는 같은 application service를 사용해야 한다. API handler와 WebSocket handler가 각각 다른 dispatch 로직을 갖지 않는다.

### 5.6 Pending Queue Drain

Agent가 새로 연결되면 해당 Agent의 pending assignment를 drain한다.

정책:

- 인증 완료 후 즉시 pending command/runbook/drift assignment를 조회한다.
- 한 번에 무제한 dispatch하지 않는다.
- 기본은 agent당 concurrent task 1개다.
- task_result를 받은 뒤 다음 pending task를 보낸다.
- high-risk task는 signed envelope와 approval 조건을 유지한다.

drain 순서:

1. expired assignment 제거 또는 expired 처리
2. command assignment 조회
3. runbook assignment 조회
4. drift check assignment 조회
5. 우선순위 정책에 따라 1개 dispatch

초기 우선순위:

- command/runbook/drift 간 명확한 제품 정책이 아직 없으므로 created_at 기준 FIFO를 우선한다.
- 현재 store API가 type별 pending 조회라면 Task 002 또는 Task 006에서 통합 pending query를 추가한다.
- 통합 query 전까지는 기존 type별 조회 순서가 bias를 만들 수 있음을 문서화하고 테스트로 고정한다.

### 5.7 Session Liveness와 Offline 판정

상시 연결 이후 online/offline 기준:

- active authenticated session 있음: online
- active session 없음, last_seen_at이 offline threshold 이내: reconnecting 또는 recently_seen
- threshold 초과: offline
- revoked/disabled: offline + revoked

필요 설정:

- `--agent-session-idle-timeout-seconds`
- `--agent-heartbeat-timeout-seconds`
- 기본값 후보:
  - heartbeat interval: 15초 또는 30초
  - missed heartbeat threshold: 3회
  - idle session timeout: 60초 또는 90초

단, 설정은 bootstrap에서만 받는다. runtime UI에서 바꾸지 않는다.

### 5.8 Revoke 즉시 반영

Agent key revoke 시:

- DB에서 agent status를 disabled로 변경
- session registry에서 해당 agent session을 찾아 close
- pending assignment는 canceled/rejected/held 정책 중 하나로 처리
- running task는 agent side에서 이미 실행 중일 수 있으므로 즉시 중단 보장은 별도 기능이다.

MVP 이후 정책:

- key revoke는 "추가 task 수신 차단"과 "현재 session 종료"를 보장한다.
- 이미 OS process로 실행 중인 command kill은 별도 cancellation protocol이 필요하다.

### 5.9 Controller Scale 한계

상시 connection은 Controller process resource를 사용한다.

초기 정책:

- 단일 Controller process 기준 in-memory session registry
- SQLite 기준 수십-수백 agent MVP/Beta 목표
- 대규모/HA는 후속 phase

관리해야 할 값:

- active sessions count
- per-session outbound queue depth
- output chunk write latency
- WebSocket read/write error count
- task dispatch latency

### 5.10 Store lock과 async boundary

현재 Controller는 `Arc<Mutex<SqliteStore>>` 형태의 store lock을 사용한다. 상시 WebSocket에서는 lock 범위를 더 엄격하게 관리해야 한다.

원칙:

- store lock을 잡은 채 `.await`하지 않는다.
- DB 조회 결과를 value로 복사한 뒤 lock을 해제하고 WebSocket send를 수행한다.
- WebSocket send 성공 후 다시 lock을 잡아 상태를 갱신한다.
- send와 status update 사이의 실패는 audit 가능한 dispatch failure로 남긴다.

금지 예:

```text
let store = lock_store(state)?;
let assignment = store.find_pending(...)?;
send_axum_wire_message(socket, assignment).await?;
store.update_job_status(...)?;
```

허용 예:

```text
let assignment = {
  let store = lock_store(state)?;
  store.find_pending(...)?
};
send_assignment(...).await?;
{
  let store = lock_store(state)?;
  store.update_job_status(...)?
}
```

## 6. Agent 변경 계획

### 6.1 Agent Loop 이름과 책임 변경

현재 이름은 heartbeat 중심이다.

변경 방향:

- `run_agent_heartbeat_loop` -> `run_agent_session_loop`
- `run_agent_heartbeat_once` -> `run_agent_session_once`
- `AgentHeartbeatOptions` -> `AgentSessionOptions`

Tidy First:

- behavior 변경 전에 이름과 option 구조를 먼저 정리한다.
- CLI backward compatibility는 유지한다.
- `--heartbeat-interval-seconds`는 liveness heartbeat interval로 의미를 재정의한다.

### 6.2 Agent 내부 loop 분리

상시 연결 Agent는 최소 다음 loop를 가진다.

```text
Agent session
  read loop
    Controller -> Agent messages

  heartbeat loop
    Agent -> Controller heartbeat

  facts loop
    Agent -> Controller facts_snapshot

  metrics loop
    Agent -> Controller metrics_snapshot

  log loop
    Agent -> Controller log_chunk

  task execution worker
    execute signed tasks
    stream output_chunk
    send task_result
```

처음부터 완전 병렬 구조가 부담되면 단계적으로 간다.

단계 1:

- WebSocket 연결을 유지한다.
- read loop에서 task_assignment를 기다린다.
- heartbeat/facts/metrics/log는 같은 thread/loop에서 interval tick으로 보낸다.
- 이 단계에서는 긴 command가 heartbeat를 막을 수 있으므로 제품 완료 상태로 보지 않는다.

단계 2:

- read/write를 분리하고 outbound mpsc queue를 둔다.
- task execution 중에도 heartbeat와 log upload가 막히지 않도록 한다.
- command output callback은 socket에 직접 쓰지 않고 outbound queue에 `output_chunk` message를 넣는다.

단계 3:

- task cancellation, concurrent task 제한, output backpressure를 넣는다.

필수 agent 내부 규칙:

- WebSocket writer는 하나만 둔다.
- task worker, heartbeat tick, facts tick, metrics tick, log tick은 모두 outbound queue로 message를 보낸다.
- read loop는 controller message를 받고 task worker에게 work item을 넘긴다.
- task worker가 오래 실행되어도 heartbeat/liveness는 계속 전송되어야 한다.
- outbound queue가 가득 차면 command output을 무제한 메모리에 쌓지 않는다.
- output limit 초과, queue overflow, write failure는 task failure 또는 session failure로 명확히 전환한다.

구현 선택지:

- 현재 sync tungstenite 기반을 유지하면 read/write/task worker를 thread와 channel로 나눈다.
- async tungstenite로 전환하면 tokio task와 mpsc channel로 나눈다.
- 어느 쪽이든 Domain/Application 계층에는 async runtime/WebSocket type을 노출하지 않는다.

### 6.3 Task 실행 정책

기본 정책:

- agent당 동시에 실행되는 high-risk task는 1개.
- task 실행 중 추가 task_assignment를 받으면 reject 또는 queue 중 선택한다.
- 초기 구현은 controller가 agent당 1개만 dispatch하도록 하고, agent는 busy 상태에서 받은 task를 `task_rejected` 또는 `security_event`로 거부한다.
- low-risk drift check와 high-risk command의 동시 실행 허용 여부는 후속 정책으로 미룬다. 초기 구현은 agent당 전체 task concurrency 1로 단순화한다.

검증 유지:

- target agent id 일치
- signature 존재
- expiry 검증
- nonce replay 방지
- controller public key 검증
- unsigned/invalid/expired/replayed/mismatch task 거부

### 6.4 Reconnect 정책

상시 연결 실패 시:

- Agent는 종료하지 않는다.
- `--once`인 경우 첫 실패에서 종료한다.
- 기본은 무한 retry.
- `--max-reconnect-attempts`가 0이면 무한 retry.
- backoff는 현재처럼 capped exponential 유지.
- reconnect 후 controller identity pinning을 다시 검증한다.

추가 정책:

- 정상 연결이 오래 유지되다가 끊긴 경우 reconnect attempt count는 reset된다.
- TLS/certificate/pinning mismatch는 보안 오류이므로 무한 retry보다 명확한 fatal failure로 볼지 결정해야 한다.

권장:

- 네트워크 오류: retry
- DNS/connection refused/timeout: retry
- auth rejected/revoked: retry하지 않고 명확히 종료 또는 긴 backoff
- controller fingerprint mismatch: fatal

session 안정화 기준:

- 연결이 일정 시간 이상 유지되면 reconnect attempt count를 reset한다.
- 짧은 시간에 반복적으로 auth rejected가 발생하면 log를 flood하지 않고 backoff를 늘린다.
- revoked agent는 계속 재시도해 controller를 압박하지 않도록 종료 또는 긴 backoff를 선택한다.
- controller fingerprint mismatch는 보안 이벤트이므로 자동 re-enroll로 해결하지 않는다.

### 6.5 Facts/Metrics/Log Interval 분리

상시 연결에서는 다음 옵션을 분리한다.

- `--heartbeat-interval-seconds`
- `--facts-interval-seconds`
- `--metrics-interval-seconds`
- `--log-upload-interval-seconds`
- `--disable-log-upload`

기본값 후보:

- heartbeat: 15초
- facts: 300초
- metrics: 30초
- log upload: 30초

이유:

- facts는 정적 inventory라 매 heartbeat마다 보낼 필요가 없다.
- metrics는 chart를 위해 30초 정도면 충분하다.
- heartbeat는 liveness만 담당한다.

## 7. Storage와 Domain/Application 변경

### 7.1 Assignment 상태 추가

현재 pending assignment는 job type별 조회로 처리된다.

현재 사실:

- `task_assignments` 테이블은 이미 존재한다.
- 현재 컬럼은 id, job_id, agent_id, nonce, payload_hash, signature, issued_at, expires_at, created_at 중심이다.
- assignment status, dispatched_at, started_at, completed_at, last_error는 아직 없다.
- `jobs.status` domain enum에는 `Dispatched`, `Rejected`가 아직 없다.

상시 연결에서 필요한 상태:

- pending
- dispatched
- running
- completed
- failed
- rejected
- expired

최소 구현:

- 기존 schema를 최대한 유지한다.
- send 성공 시 job status를 running으로 변경한다.
- task_result에서 success/failed로 변경한다.
- assignment 개별 상태는 후속 migration으로 분리한다.

권장 후속:

```text
task_assignments
  job_id
  task_id
  agent_id
  status
  dispatched_at
  started_at
  completed_at
  last_error
```

권장 실제 순서:

1. Domain `JobStatus`에 `Rejected` 추가 여부를 먼저 결정하고 테스트한다.
2. `Dispatched`/`Accepted`는 payload가 추가되기 전까지 보류하거나, `Running`으로 coarse하게 표현한다.
3. `task_assignments.status` migration을 추가한다.
4. repository contract에 `mark_assignment_dispatched`, `mark_assignment_started`, `mark_assignment_completed`, `mark_assignment_failed`, `mark_assignment_rejected`를 추가한다.
5. Web Admin/API에는 job status와 assignment status를 혼동하지 않도록 별도 필드로 노출한다.

초기 제품 판단:

- "send 성공"은 엄밀히 말하면 "Agent가 실행 시작"이 아니다.
- 1차 구현에서 새 `task_started` payload를 만들지 않는다면 send 성공을 `running`으로 표현할 수 있으나, 문서에는 "delivered to active session" 수준의 coarse 상태라고 명시한다.
- 2차 구현에서 `task_ack`/`task_started`를 추가하면 `dispatched`와 `running`을 분리한다.

### 7.2 Outbox Pattern 검토

즉시 dispatch는 DB transaction과 WebSocket send 사이의 불일치 위험이 있다.

위험:

- DB에는 job이 저장됐지만 send 실패
- send 성공했지만 status update 실패
- Controller crash로 dispatch 시도 중단

초기 정책:

- DB queued 저장을 source of truth로 둔다.
- send 실패 시 queued 유지.
- send 성공 후 status update 실패는 audit/error log로 남기고 task_result가 오면 최종 상태를 회복한다.

후속:

- dispatch outbox table
- background dispatcher
- idempotent task assignment delivery

운영 기준:

- 상시 연결의 핵심은 "connected agent 즉시 push"지만, source of truth는 여전히 DB다.
- Controller restart 후에도 queued assignment를 잃지 않아야 한다.
- running 상태였으나 task_result가 없는 job은 startup recovery 또는 periodic reconciler에서 `expired`/`unknown`/`failed` 정책을 적용해야 한다.
- MVP 이후 제품화 단계에서는 outbox 또는 reconciler 중 하나를 반드시 둔다.

### 7.3 Idempotency와 Replay

상시 연결은 reconnect와 duplicate send 가능성이 있다.

필수:

- task envelope nonce는 계속 유지한다.
- agent nonce replay guard는 process lifetime뿐 아니라 persistent guard 필요 여부를 검토한다.
- task_result 중복 수신은 idempotent해야 한다.
- output_chunk 중복은 `(job_id, agent_id, sequence)` uniqueness로 방어한다.

현재 output chunk uniqueness가 있으면 유지하고 테스트를 강화한다.

현재 store 기준 보완:

- 실제 unique constraint는 `(job_id, agent_id, stream, chunk_index)`다.
- 따라서 stdout sequence 0과 stderr sequence 0은 동시에 저장될 수 있다.
- duplicate output insert는 constraint violation이므로 controller가 전체 session을 죽일지, duplicate를 idempotent success로 볼지 정책을 정해야 한다.
- 권장 정책은 같은 `(job_id, agent_id, stream, chunk_index)`의 중복은 idempotent duplicate로 보고 FieldDebug에만 남기는 것이다. 단 body가 다르면 security/audit 대상이다.

### 7.4 Session summary 저장 여부

active session은 runtime state지만 운영자는 상태를 조회해야 한다.

정책:

- active session 자체는 DB에 저장하지 않는다.
- `agents.last_seen_at`은 heartbeat/facts/metrics/log/task_result 등 authenticated activity 시 갱신한다.
- API 응답에는 DB 상태와 runtime session summary를 조합해서 반환한다.
- Controller restart 직후 active session summary는 비어 있을 수 있으며, Agent reconnect로 회복한다.

필요 API 필드 후보:

- `connected`: boolean
- `connection_id`: field-debug 또는 admin detail 전용
- `connected_at_ms`
- `last_session_seen_at_ms`
- `session_capabilities`
- `running_job_id`

## 8. Security/Audit 변경

### 8.1 Authenticated Session Boundary

상시 연결에서 가장 중요한 경계:

- 인증 전에는 task/data payload를 처리하지 않는다.
- auth accepted 이후 session registry에 등록한다.
- registry에 등록된 session만 task_assignment를 받을 수 있다.
- revoked agent는 registry에 남아 있으면 안 된다.
- 인증된 session에서 수신한 모든 payload의 agent_id는 session agent_id와 일치해야 한다.
- 일치하지 않는 facts/metrics/log/output/drift/security_event는 처리하지 않고 security audit 대상이다.
- task_assignment는 controller-signed envelope가 있는 경우에만 agent가 실행한다.
- controller는 active session이 있다는 이유만으로 approval/high-risk 정책을 우회하지 않는다.

감사 이벤트:

- `agent_session_started`
- `agent_session_ended`
- `agent_session_replaced`
- `agent_session_auth_failed`
- `agent_session_revoked_closed`
- `task_dispatched`
- `task_dispatch_failed`
- `task_rejected`

Product log:

- session started/ended는 너무 많으면 noisy할 수 있으므로 상태 변화 중심으로 제한한다.
- command stdout/stderr 원문은 Product log에 남기지 않는다.
- dispatch success/failure는 job id, agent id, status, latency 중심으로 남긴다.

Field debug:

- connection id, close reason, queue depth, dispatch latency 포함.
- protocol message type, payload size, redacted metadata를 포함할 수 있다.
- raw task payload, raw token, private key, command output 전체 dump는 금지한다.

### 8.2 Revoked Agent

상시 연결에서 revoke는 즉시성이 필요하다.

정책:

- revoke API 성공 직후 active session close.
- revoked agent가 heartbeat/log/output을 보내면 무시하고 security audit.
- revoked agent가 재연결하면 auth 단계에서 거부.
- UI에는 offline + revoked를 동시에 표시.
- revoke 직전 queued assignment는 dispatch하지 않는다.
- revoke 직전 running job은 즉시 kill을 보장하지 않는다. 이 한계는 UI/API 문서에 명확히 쓴다.
- task cancellation protocol이 추가되기 전에는 timeout을 running process boundary로 사용한다.

### 8.3 Agent command execution boundary

상시 연결은 command 실행 반응성을 높이지만 실행 권한 정책을 완화하지 않는다.

필수:

- command timeout 유지
- max output bytes 유지
- high-risk confirmation/approval 유지
- signed envelope 검증 유지
- controller public key pinning 유지
- root 실행 boundary 유지

금지:

- 즉시성을 위해 shell string을 그대로 실행하는 shortcut
- busy agent에 무제한 task를 밀어 넣는 방식
- output backpressure를 무시하고 메모리에 계속 쌓는 방식
- 실패한 signature 검증을 retry 대상으로 취급하는 방식

### 8.4 HTTP Transport

상시 연결에서도 HTTP/WSS 정책은 유지한다.

- HTTP/WS는 테스트 전용.
- HTTPS/WSS는 제품/운영 기본.
- HTTP 사용 시 controller와 agent 양쪽에 경고 출력.
- HTTP external URL이면 Security audit에 남김.

## 9. Web Admin 변경

### 9.1 Agent 상태 표시

Agent list/detail에 다음 정보를 표시한다.

- connected / reconnecting / offline / revoked
- active session 여부
- last heartbeat time
- last facts time
- last metrics time
- current running job 여부

현재 `online` 하나로는 부족하다.

### 9.2 Run UX

Run 패널은 다음 상태를 구분해야 한다.

- job created
- queued because agent offline
- dispatched to active session
- running
- output streaming
- completed success/failed
- expired
- rejected

문구 정책:

- `No job output`은 polling 완료 후에도 chunk가 전혀 없을 때만 사용한다.
- 아직 처리 전이면 `Waiting for agent to accept the job`.
- agent offline이면 `Queued until agent reconnects`.
- task 전달 성공 후이면 `Running on agent`.

### 9.3 Output 갱신 방식

초기:

- 기존 `/api/jobs/{job_id}/output` polling 유지
- polling interval은 짧게 조정 가능
- job status도 같이 polling
- 최소한 `/api/jobs` 응답 또는 신규 `/api/jobs/{job_id}` 응답에 현재 job status, target agent status, dispatch state를 포함한다.

후속:

- Admin UI용 SSE 또는 WebSocket output subscribe 추가
- REST OpenAPI에는 output list API를 유지
- Web Admin UI는 subscribe 가능하면 realtime, 실패하면 polling fallback

### 9.4 API 계약 보강

Web Admin이 상태를 추측하지 않게 API를 보강한다.

필요 후보:

```http
GET /api/jobs/{job_id}
GET /api/agents/{agent_id}/session
```

`GET /api/jobs/{job_id}` 응답 후보:

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

`GET /api/agents/{agent_id}/session` 응답 후보:

```json
{
  "agent_id": "agent-1",
  "connected": true,
  "connected_at_ms": 1710000000000,
  "last_seen_at_ms": 1710000005000,
  "capabilities": ["persistent_session"]
}
```

규칙:

- Web Admin은 `connected`나 `dispatch_state`를 자체 계산하지 않는다.
- API 예시에는 token, raw command output, private key를 넣지 않는다.
- Swagger/OpenAPI와 `web-admin/api.schema.json`을 함께 갱신한다.

## 10. CLI/문서 변경

### 10.1 CLI help

`sponzey agent start --help` 설명을 바꾼다.

기존:

- heartbeat and task loop

변경:

- persistent agent session
- heartbeat interval은 liveness tick
- facts/metrics/log interval 옵션 분리
- connection failure는 기본 무한 retry

### 10.2 README 변경

문서에서 다음을 명확히 한다.

- Controller가 Agent로 접속하지 않는다.
- Agent가 Controller에 outbound persistent WebSocket을 유지한다.
- `Run`은 connected Agent에 즉시 push된다.
- offline Agent는 reconnect 후 queued job을 받는다.
- HTTP는 테스트 전용, 제품/운영은 HTTPS/WSS 사용.

### 10.3 Protocol 문서 변경

`docs/protocol.md`를 업데이트한다.

- heartbeat는 liveness message로 재정의
- session lifecycle 추가
- task dispatch 즉시성 설명
- session close/reconnect 정책
- duplicate session 정책

## 11. 개발 단계

각 단계는 2-3개 기능 단위로 묶는다.

상세 실행 파일:

- [Task 001 - 현재 WebSocket Lifecycle 정리와 테스트 고정](task001.md)
- [Task 002 - Domain/Application/Store Dispatch 계약 정리](task002.md)
- [Task 003 - Controller Active Session Registry](task003.md)
- [Task 004 - Controller WebSocket Read/Write Loop 분리](task004.md)
- [Task 005 - Agent Persistent Session Loop](task005.md)
- [Task 006 - 즉시 Task Dispatch](task006.md)
- [Task 007 - Job 상태와 Output UX 정밀화](task007.md)
- [Task 008 - Facts/Metrics/Log Interval 분리](task008.md)
- [Task 009 - Revoke/Offline/Session Close 정책](task009.md)
- [Task 010 - Backpressure와 Output 안정성](task010.md)
- [Task 011 - 문서, Swagger, Smoke](task011.md)

### Task 001 - 현재 WebSocket Lifecycle 정리와 테스트 고정

목표: behavior 변경 전에 현재 구조의 문제를 테스트로 고정하고 이름/경계를 정리한다.

기능:

- [ ] 현재 heartbeat 단발 WebSocket 흐름을 integration/unit test로 명확히 기록한다.
- [ ] Agent loop 함수/옵션 이름을 session 중심으로 정리할 준비를 한다.
- [ ] protocol 문서에 현재 한계와 전환 목표를 명시한다.

검증:

- [ ] `cargo test -p fleet-controller websocket`
- [ ] `cargo test -p fleet-cli agent_heartbeat_loop`
- [ ] `cargo fmt --all --check`

완료 기준:

- [ ] 현재 동작이 깨지지 않는다.
- [ ] 다음 task에서 상시 연결을 넣어도 회귀 확인이 가능하다.

### Task 002 - Domain/Application/Store Dispatch 계약 정리

목표: session 구현 전에 job/assignment 상태와 dispatch use case 계약을 먼저 고정한다.

기능:

- [ ] 현재 `JobStatus`와 `task_assignments` schema의 차이를 반영해 1차 상태 모델을 결정한다.
- [ ] `DispatchPendingAssignments` application service contract를 정의한다.
- [ ] assignment status migration 또는 coarse `running` 상태 유지 중 1차 구현 범위를 결정한다.

검증:

- [ ] domain job transition test
- [ ] repository assignment query/status contract test
- [ ] duplicate output chunk/idempotency policy test

완료 기준:

- [ ] API/WebSocket handler가 직접 dispatch business rule을 갖지 않는다.
- [ ] DB queued assignment가 source of truth임이 테스트로 보장된다.
- [ ] `send success`와 `task started`의 의미 차이를 문서화한다.

### Task 003 - Controller Active Session Registry

목표: 인증된 Agent WebSocket session을 Controller가 추적할 수 있게 한다.

기능:

- [ ] `AgentSessionRegistry`를 추가한다.
- [ ] authenticated session 등록/해제 lifecycle을 구현한다.
- [ ] duplicate session은 new session wins 정책으로 처리한다.
- [ ] session summary DTO를 정의하되 DB에 session handle을 저장하지 않는다.

검증:

- [ ] session 등록/해제 unit test
- [ ] duplicate session replacement test
- [ ] revoked agent가 session registry에 남지 않는 test
- [ ] registry operation이 store lock과 섞이지 않는 구조 검토

완료 기준:

- [ ] Controller가 active agent session 여부를 알 수 있다.
- [ ] session registry는 DB가 아니라 runtime state로 유지된다.

### Task 004 - Controller WebSocket Read/Write Loop 분리

목표: Controller가 Agent session으로 task를 나중에 push할 수 있게 만든다.

기능:

- [ ] WebSocket을 read half/write half로 분리한다.
- [ ] per-agent outbound task channel을 둔다.
- [ ] channel overflow와 write failure 처리 정책을 구현한다.
- [ ] store lock을 잡은 상태에서 `.await`하지 않도록 handler 구조를 정리한다.

검증:

- [ ] outbound channel로 task_assignment를 send하는 test
- [ ] write failure 시 session cleanup test
- [ ] channel overflow 시 queued 유지 또는 dispatch failure audit test
- [ ] store lock 범위가 WebSocket await와 겹치지 않는 code review checklist 반영

완료 기준:

- [ ] Agent가 idle 상태로 연결을 유지할 수 있다.
- [ ] Controller 내부에서 active session에 message push가 가능하다.

### Task 005 - Agent Persistent Session Loop

목표: Agent가 heartbeat마다 연결을 닫지 않고 상시 연결을 유지한다.

기능:

- [ ] `run_agent_session_loop`를 구현한다.
- [ ] 인증 후 read loop에서 task_assignment를 계속 기다린다.
- [ ] heartbeat를 liveness tick으로 주기 전송한다.
- [ ] WebSocket writer를 단일 outbound queue로 통합한다.

검증:

- [ ] session loop reconnect test
- [ ] heartbeat interval test
- [ ] controller close 후 retry test
- [ ] 긴 task 실행 중 heartbeat가 막히지 않는 test

완료 기준:

- [ ] Agent는 정상 상태에서 연결을 유지한다.
- [ ] connection failure 시 기본적으로 종료하지 않고 재시도한다.
- [ ] task worker가 socket에 직접 동시에 쓰지 않는다.

### Task 006 - 즉시 Task Dispatch

목표: `Run` 요청 직후 connected Agent에 task를 즉시 전달한다.

기능:

- [ ] job 생성 후 active session dispatch를 시도한다.
- [ ] connected agent는 즉시 task_assignment를 받는다.
- [ ] disconnected agent는 queued 상태로 유지한다.
- [ ] agent session 등록 직후 pending queue drain을 수행한다.

검증:

- [ ] connected agent에 command job 즉시 dispatch integration test
- [ ] disconnected agent job queued test
- [ ] send 실패 시 queued 유지 또는 audit test
- [ ] reconnect 후 queued job drain test

완료 기준:

- [ ] heartbeat interval과 무관하게 Run 결과가 시작된다.
- [ ] Web Admin의 `Run` 지연은 command 실행 시간과 output polling 시간으로만 제한된다.

### Task 007 - Job 상태와 Output UX 정밀화

목표: 운영자가 job이 어디서 멈췄는지 알 수 있게 한다.

기능:

- [ ] job 상태에 dispatch/running/rejected/expired 의미를 정리한다.
- [ ] `/api/jobs` 또는 job detail API에서 dispatch 상태를 확인할 수 있게 한다.
- [ ] Web Admin Run 패널의 상태 문구를 상태별로 분리한다.
- [ ] Web Admin이 상태를 추측하지 않도록 API response를 우선 보강한다.

검증:

- [ ] job status transition test
- [ ] Web Admin smoke test
- [ ] output 없는 completed job과 pending job 문구 구분 test
- [ ] OpenAPI와 `web-admin/api.schema.json` 정합성 test

완료 기준:

- [ ] `No job output`은 실제로 output이 없을 때만 표시된다.
- [ ] offline/queued/running/completed가 UI에서 구분된다.

### Task 008 - Facts/Metrics/Log Interval 분리

목표: telemetry와 task dispatch 반응성을 분리한다.

기능:

- [ ] facts interval 옵션 추가
- [ ] metrics interval 옵션 추가
- [ ] log upload interval은 기존 정책 유지하되 session loop에 통합

검증:

- [ ] facts가 매 heartbeat마다 오지 않는 test
- [ ] metrics는 설정 interval에 맞춰 전송되는 test
- [ ] log upload disable/interval test 유지

완료 기준:

- [ ] task push는 telemetry 주기와 무관하다.
- [ ] facts는 정적 inventory로 낮은 빈도 전송된다.
- [ ] metrics는 chart에 필요한 빈도로 전송된다.

### Task 009 - Revoke/Offline/Session Close 정책

목표: 상시 연결에서 보안 이벤트가 즉시 반영되게 한다.

기능:

- [ ] revoke agent key 시 active session을 즉시 close한다.
- [ ] session close reason을 audit/log에 남긴다.
- [ ] offline 판정을 active session + heartbeat timeout 기준으로 재정의한다.
- [ ] revoked agent의 queued/running job 정책을 문서와 UI에 반영한다.

검증:

- [ ] revoke API가 active session을 제거하는 test
- [ ] revoked agent reconnect 거부 test
- [ ] active session close 후 agent inventory offline+revoked 표시 test
- [ ] revoke 직후 새 task dispatch가 막히는 test

완료 기준:

- [ ] revoked agent는 더 이상 task를 받을 수 없다.
- [ ] UI는 revoked/offline을 명확히 보여준다.

### Task 010 - Backpressure와 Output 안정성

목표: 상시 연결에서 output과 outbound task가 Controller를 압박하지 않도록 한다.

기능:

- [ ] per-session outbound queue 크기 제한
- [ ] output chunk size/max output 정책 재검증
- [ ] long-running command timeout과 connection drop 처리

검증:

- [ ] output limit exceeded test
- [ ] slow store/write failure test
- [ ] running 중 disconnect 시 job 상태 정책 test

완료 기준:

- [ ] Agent 하나의 과도한 output이 Controller 전체를 막지 않는다.
- [ ] output은 Product application log로 흐르지 않고 job output storage에만 저장된다.

### Task 011 - 문서, Swagger, Smoke

목표: 실제 사용자가 상시 연결 모델을 이해하고 검증할 수 있게 한다.

기능:

- [ ] README.md / README.ko.md 업데이트
- [ ] docs/protocol.md 업데이트
- [ ] docs/api.md와 OpenAPI job status 설명 업데이트

검증:

- [ ] local controller + agent + run immediate smoke script
- [ ] remote HTTP warning smoke 유지
- [ ] HTTPS/WSS smoke 유지

완료 기준:

- [ ] 문서가 "Controller가 Agent로 접속한다"는 오해를 만들지 않는다.
- [ ] 사용자는 connected Agent에서 Run 결과가 즉시 나오는 것을 확인할 수 있다.

## 12. 구현 순서

권장 순서:

1. Task 001로 현재 동작을 테스트와 문서로 고정한다.
2. Task 002로 domain/application/store dispatch 계약을 먼저 고정한다.
3. Task 003으로 Controller session registry를 만든다.
4. Task 004로 Controller WebSocket writer push 경로를 만든다.
5. Task 005로 Agent persistent session을 만든다.
6. Task 006으로 job 생성 후 즉시 dispatch를 연결한다.
7. Task 007로 UI와 job status를 정리한다.
8. Task 008로 telemetry interval을 분리한다.
9. Task 009로 revoke/offline 정책을 보안 관점에서 완성한다.
10. Task 010으로 backpressure와 failure mode를 정리한다.
11. Task 011로 문서와 smoke를 닫는다.

## 13. 주요 리스크와 대응

### 13.1 WebSocket write와 DB 상태 불일치

리스크:

- task를 보냈지만 DB status update가 실패할 수 있다.

대응:

- DB queued 저장을 source of truth로 둔다.
- send 실패 시 queued 유지.
- send 성공 후 status update 실패는 audit/error log.
- 후속 outbox pattern 검토.

### 13.2 Controller restart

리스크:

- active session registry는 메모리 상태라 restart 시 사라진다.

대응:

- Agent reconnect loop가 source of recovery다.
- queued/running job 상태 복구 정책을 정한다.
- startup recovery에서 오래된 running job을 expired 또는 unknown으로 전환하는 task를 후속으로 둔다.

### 13.3 Duplicate task execution

리스크:

- reconnect 중 같은 assignment가 두 번 전달될 수 있다.

대응:

- signed envelope nonce 유지.
- agent nonce replay guard 강화.
- output chunk uniqueness 유지.
- task_result idempotency 추가.

### 13.4 Revoke와 running process

리스크:

- session을 닫아도 이미 실행 중인 local process가 계속 돌 수 있다.

대응:

- revoke는 "추가 task 차단"과 "session 종료"로 정의한다.
- running process cancel은 별도 cancellation protocol로 다룬다.
- dangerous/high-risk task는 timeout을 강제한다.

### 13.5 Resource 증가

리스크:

- 상시 connection은 Controller file descriptor, memory, task queue를 사용한다.

대응:

- max active session 수 설정 검토.
- per-session queue bound.
- output size limit 유지.
- metrics/log로 session count와 queue depth 노출.

## 14. 완료 기준

상시 연결 전환 완료 기준:

- [ ] Agent는 정상 상태에서 Controller와 WebSocket을 상시 유지한다.
- [ ] Controller는 active session registry를 가진다.
- [ ] session registry는 runtime state이며 WebSocket handle을 DB에 저장하지 않는다.
- [ ] Web Admin `Run`은 connected Agent에 즉시 task를 push한다.
- [ ] heartbeat interval을 30초로 둬도 Run 결과가 다음 heartbeat까지 지연되지 않는다.
- [ ] disconnected Agent는 queued job을 reconnect 후 받는다.
- [ ] revoked Agent session은 즉시 닫힌다.
- [ ] facts/metrics/log 전송 주기는 task dispatch와 분리된다.
- [ ] job status 또는 dispatch_state가 queued/delivered/running/success/failed/expired/rejected를 운영자가 구분할 수 있게 노출된다.
- [ ] output chunk는 계속 job output storage에만 저장되고 Product application log에는 원문이 남지 않는다.
- [ ] command 실행 중에도 heartbeat/liveness가 막히지 않는다.
- [ ] store lock을 잡은 상태에서 WebSocket await를 수행하지 않는다.
- [ ] HTTP transport 경고 정책은 유지된다.
- [ ] README.md, README.ko.md, docs/protocol.md, docs/api.md, OpenAPI가 실제 동작과 일치한다.
- [ ] smoke test에서 `Run` 직후 즉시 output이 관찰된다.

## 15. 이번 계획에서 하지 않을 것

- Controller가 Agent로 직접 inbound 접속하는 구조
- Agent별 포트 오픈 요구
- SSH 기반 fallback을 제품 경로로 승격
- runtime UI에서 controller 설정/env를 변경하는 기능
- unsigned task 허용
- high-risk command confirmation 우회
- Web Admin에 domain rule을 중복 구현
- multi-controller HA session migration
- full streaming admin WebSocket을 첫 단계에서 필수화

## 16. 다음 액션

바로 구현에 들어가려면 Task 001부터 진행한다.

첫 구현 목표는 다음 한 문장으로 검증한다.

```text
Agent가 이미 Controller에 연결되어 있다면, Web Admin에서 Run을 누른 직후 heartbeat 주기와 무관하게 task_assignment가 Agent로 전달되어야 한다.
```