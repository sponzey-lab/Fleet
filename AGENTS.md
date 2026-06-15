# Sponzey Fleet 개발 에이전트 규칙

이 문서는 Sponzey Fleet 프로젝트에서 코드를 작성하거나 수정하는 모든 사람과 자동화 에이전트가 따라야 하는 작업 규칙이다.  
프로젝트의 기본 방향은 `PROJECT.md`를 따른다. 핵심은 Rust core 기반 Controller/Agent/CLI와 가벼운 Web Admin UI다.

## 1. 프로젝트 정체성

Sponzey Fleet는 agent 기반 실시간 서버 운영 자동화 플랫폼이다.

핵심 구성:

- Rust core
- 단일 Rust 바이너리 `sponzey`
- `sponzey controller ...` 역할
- `sponzey agent ...` 역할
- `sponzey` CLI 역할
- WebSocket over TLS 기반 outbound agent 연결
- Agent가 Controller에 유지하는 persistent outbound WebSocket session
- SQLite/Postgres storage
- 가벼운 Web Admin UI
- npm 설치 UX는 유지하되, npm package는 Rust 바이너리 배포 wrapper로 사용

이 프로젝트는 단순 원격 쉘 도구가 아니다. root 권한 실행, 상태 수집, drift detection, remediation, audit log를 다루는 운영 플랫폼이다. 따라서 구조, 테스트, 설정, 로그, 보안 정책을 처음부터 엄격하게 가져간다.

## 2. 최상위 기초룰

아래 규칙은 모든 코드, 문서, 테스트, 배포 스크립트에 우선 적용한다.

1. Layered Architecture, Clean Architecture, Tidy First, TDD를 반드시 적용한다.
2. 외부 파일에 설정되는 내용은 최소화한다.
3. 환경 설정 내용을 프로세스 중간에 삽입하여 변경하는 방법은 반드시 거부한다.
4. 외부 환경 상수는 최초 부팅 시점에만 받아들이고, 이후에는 프로그램 상수가 아니라 명시적 인자, 설정 객체, 요청 payload, command argument로만 전달한다.
5. 로그는 3가지 수준으로 나눈다.
   - 프로덕트용 최소 로그
   - 현장 확인용 디버그 로그
   - 개발 및 테스트용 개발 로그

이 규칙은 편의보다 우선한다. 구현이 조금 길어지더라도 전역 상태, 암묵적 설정, 중간 환경 변경, 테스트 없는 핵심 로직 추가를 허용하지 않는다.

## 3. 아키텍처 원칙

### 3.1 Layered Architecture

코드는 다음 계층을 기준으로 나눈다.

```text
Interface Layer
  CLI
  HTTP API
  WebSocket gateway
  Web Admin UI static serving

Application Layer
  use cases
  command handlers
  job orchestration
  enrollment flow
  drift check flow
  remediation flow

Domain Layer
  entities
  value objects
  policies
  task model
  job state machine
  agent state machine
  domain errors

Infrastructure Layer
  SQLite/Postgres repository
  filesystem
  process runner
  systemd/journald adapter
  network transport
  TLS/certificate store
  clock/random/id generator
```

의존 방향:

```text
Interface -> Application -> Domain
Infrastructure -> Application/Domain contracts
Domain -> no outer layer dependency
```

절대 금지:

- Domain layer에서 DB, HTTP, WebSocket, filesystem, environment variable 직접 접근
- Application layer에서 `std::env` 직접 접근
- CLI argument parsing 결과를 domain object 없이 그대로 infrastructure로 전달
- HTTP handler 안에 business logic 작성
- Agent task runner 안에 policy 판단 로직 작성

### 3.2 Clean Architecture

핵심 비즈니스 규칙은 framework와 분리한다.

예시:

- `fleet-domain`: `Agent`, `Job`, `Runbook`, `Policy`, `DriftReport`, `AuditEvent`
- `fleet-application`: `EnrollAgent`, `DispatchJob`, `CollectFacts`, `CheckDrift`, `ApproveJob`
- `fleet-infra`: `SqliteAgentRepository`, `PostgresJobRepository`, `TokioProcessRunner`
- `fleet-controller`: HTTP/WebSocket controller library
- `fleet-agent`: agent daemon library
- `fleet-cli`: 단일 `sponzey` binary entrypoint와 subcommand UX

Application service는 trait으로 필요한 기능을 받는다.

```rust
pub trait AgentRepository {
    async fn save(&self, agent: Agent) -> Result<(), StoreError>;
    async fn find_by_id(&self, id: AgentId) -> Result<Option<Agent>, StoreError>;
}

pub trait AuditWriter {
    async fn write(&self, event: AuditEvent) -> Result<(), AuditError>;
}
```

좋은 구조:

```text
HTTP request -> DTO validation -> use case input -> application service -> domain -> repository trait
```

나쁜 구조:

```text
HTTP request -> SQL query -> mutable global config -> shell command -> ad hoc log
```

### 3.3 Tidy First

기능 변경과 구조 정리를 섞지 않는다. Tidy First 원칙에 따라 다음 순서를 지킨다.

1. 먼저 작은 구조 정리를 한다.
2. 구조 정리 commit 또는 작업 단위를 기능 변경과 분리한다.
3. 이후 기능 변경을 한다.
4. 기능 변경에는 테스트를 붙인다.

허용되는 tidy 작업:

- 이름을 명확히 바꾸기
- 중복된 작은 helper 추출
- trait boundary 정리
- module 이동
- error type 명확화
- 테스트 fixture 정리

금지되는 tidy 작업:

- 기능 구현 중 대규모 폴더 재배치
- 테스트 없이 behavior가 바뀌는 정리
- unrelated formatting churn
- “나중에 필요할 것 같아서” 만드는 추상화

기준:

- Tidy는 behavior를 바꾸지 않는다.
- behavior가 바뀌면 feature/fix 작업이다.
- 한 PR/작업 단위에서 tidy와 feature를 섞어야 한다면 커밋 또는 섹션을 분리한다.

### 3.4 TDD

핵심 로직은 테스트를 먼저 작성한다.

필수 TDD 대상:

- domain state machine
- job dispatch
- agent enrollment
- token validation
- signed task validation
- approval decision
- drift detection
- runbook parser
- selector matching
- audit event creation
- log redaction
- config parsing

권장 흐름:

1. 실패하는 테스트 작성
2. 최소 구현
3. 리팩터링
4. edge case 테스트 추가

테스트 없이 구현해도 되는 경우:

- 문서 수정
- 단순 화면 텍스트 수정
- 명백한 typo 수정
- 아직 동작 코드가 없는 계획 문서 작성

단, root 권한 실행, credential, token, TLS, audit, command dispatch 관련 코드는 테스트 없이 추가하지 않는다.

## 4. Rust Workspace 기준 구조

권장 구조:

```text
crates/
  fleet-core/
  fleet-domain/
  fleet-application/
  fleet-protocol/
  fleet-store/
  fleet-runner/
  fleet-controller/
  fleet-agent/
  fleet-cli/
web-admin/
  src/
  dist/
npm/
  fleet/
  fleet-darwin-arm64/
  fleet-linux-x64/
docs/
```

각 crate 책임:

- `fleet-domain`: 순수 domain model과 business rule
- `fleet-application`: use case orchestration
- `fleet-protocol`: agent-controller message schema
- `fleet-store`: DB schema, migration, repository 구현
- `fleet-runner`: command/package/service/file primitive 실행
- `fleet-controller`: `sponzey controller`가 사용하는 API server, WebSocket gateway, scheduler library
- `fleet-agent`: `sponzey agent`가 사용하는 daemon, local collectors, local task execution library
- `fleet-cli`: 단일 제품 바이너리 `sponzey`와 command line UX
- `web-admin`: 얇은 Web Admin UI
- `npm`: Rust 바이너리 배포 wrapper

crate 의존 규칙:

```text
fleet-domain
  no project crate dependency

fleet-application
  depends on fleet-domain

fleet-protocol
  depends on fleet-domain only if protocol shares domain value types intentionally

fleet-store
  depends on fleet-domain, fleet-application contracts

fleet-runner
  depends on fleet-domain task types

fleet-controller
  depends on application, store, protocol

fleet-agent
  depends on protocol, runner, domain

fleet-cli
  depends on protocol/client types
```

바이너리 정책:

- 제품 배포 바이너리는 `sponzey` 하나만 둔다.
- Controller와 Agent 역할은 별도 실행 파일이 아니라 `sponzey controller ...`, `sponzey agent ...` subcommand로 선택한다.
- `fleet-controller`와 `fleet-agent` crate는 library crate로 유지하고 독립 `main.rs`를 만들지 않는다.
- npm wrapper, systemd unit, release artifact는 모두 resolved absolute path의 `sponzey` 바이너리를 참조한다.
- Agent 실행 모델은 Controller로의 outbound persistent session을 기본으로 한다. Controller가 Agent로 직접 inbound 접속하는 구조를 기본 제품 경로로 만들지 않는다.

금지:

- `fleet-domain`이 `tokio`, `sqlx`, `axum`, `reqwest`, `tracing_subscriber`에 직접 의존
- `fleet-controller` 내부 model을 agent가 직접 import
- UI TypeScript type을 Rust source of truth보다 우선
- protocol schema를 여러 곳에서 수동 중복 정의

## 5. 설정 원칙

### 5.1 설정 최소화

외부 파일에 저장되는 설정은 최소화한다.

허용되는 설정 파일:

- controller persistent config
- agent identity/config
- CLI user profile
- test fixture
- migration file

허용되지 않는 설정 파일:

- 기능 토글을 임의로 늘리는 별도 YAML
- 운영 중 수동으로 수정해야만 동작하는 숨은 설정 파일
- domain rule을 외부 파일에 흩뿌리는 방식
- 테스트에서만 통과하는 ad hoc config

원칙:

- 기본값은 코드에 명확히 둔다.
- 바꿔야 하는 값만 config로 노출한다.
- config key는 문서화한다.
- deprecated config는 migration 경로를 둔다.

### 5.2 프로세스 중간 환경 변경 금지

프로세스가 시작된 뒤 환경 변수를 주입하거나 바꿔 동작을 변경하는 방식을 거부한다.

금지 예시:

```rust
std::env::set_var("FLEET_LOG_LEVEL", "debug");
std::env::set_var("DATABASE_URL", new_url);
std::env::remove_var("FLEET_CONFIG");
```

금지되는 패턴:

- 테스트 중 `set_var`로 production code behavior 변경
- request handler에서 env var 읽기
- job 실행 중 env var를 바꿔 controller 설정 변경
- Web Admin UI 요청에 따라 process env 변경
- agent task 실행 전 global env를 변경

예외:

- child process에 전달하는 per-command environment는 허용한다. 단, 전역 process env가 아니라 `Command` builder에 명시적으로 넣어야 한다.

허용 예시:

```rust
Command::new("ansible-playbook")
    .env("ANSIBLE_FORCE_COLOR", "1")
    .arg("site.yml");
```

### 5.3 외부 환경 상수는 최초에만 수집

외부 환경 상수는 process bootstrap에서 한 번만 읽는다.

허용:

```text
process start
  -> read env/args/config
  -> build Settings
  -> validate Settings
  -> freeze Settings inside AppContext
  -> pass AppContext by explicit reference
```

금지:

```text
handler
  -> std::env::var("DATABASE_URL")
  -> connect DB
```

`Settings`는 불변 객체로 다룬다.

권장 Rust 형태:

```rust
#[derive(Clone, Debug)]
pub struct Settings {
    pub bind_addr: SocketAddr,
    pub database_url: DatabaseUrl,
    pub log_profile: LogProfile,
    pub data_dir: PathBuf,
}
```

설정 전달 방식:

- function argument
- typed settings object
- application context
- explicit request payload
- CLI argument

금지:

- lazy global env lookup
- mutable singleton config
- once_cell에 넣고 내부 값을 교체
- runtime config patch endpoint
- 숨은 static mut

### 5.4 CLI/Controller/Agent 설정 경계

Controller:

- 시작 시점에 bind address, database, data directory, external URL, TLS path를 받는다.
- 실행 중 변경은 API로 직접 process state를 바꾸지 않는다.
- 변경이 필요하면 저장 후 restart 또는 explicit reload command를 설계한다.

Agent:

- enrollment 시 controller URL, agent identity, labels, service options를 저장한다.
- agent identity는 임의 수정 불가다.
- labels 변경은 controller API를 통해 audit와 함께 수행한다.

CLI:

- `~/.sponzey/config.toml`에는 user profile과 controller endpoint 정도만 둔다.
- command별 옵션은 CLI 인자로 받는다.
- CLI가 controller/agent 내부 config file을 직접 수정하지 않는다.

Web Admin UI:

- UI는 설정 편집기가 아니다.
- MVP에서는 runtime configuration 변경 기능을 만들지 않는다.
- 정책, runbook, labels, approval 같은 domain object만 다룬다.

## 6. 로그 정책

로그는 목적에 따라 3가지 수준으로 나눈다.

### 6.1 Product 로그

목적:

- 운영 환경에서 항상 켜둘 수 있는 최소 로그
- 고객에게 노출되어도 안전한 수준
- 장애 발생 시 high-level event를 추적

특징:

- 기본값
- 낮은 볼륨
- secret 없음
- command output 원문 없음
- 개인 정보 없음
- request body 원문 없음

포함:

- controller started/stopped
- agent enrolled/disabled
- agent online/offline transition
- job created/started/completed/failed
- approval requested/approved/rejected
- drift detected/resolved
- audit write failure

금지:

- token
- password
- private key
- full command output
- full HTTP body
- environment dump
- stack trace flood

예시:

```text
INFO job_completed job_id=... status=success target_count=12 changed_count=3 duration_ms=9201
WARN agent_offline agent_id=... last_seen_age_sec=93
ERROR audit_write_failed event_id=... store=postgres
```

### 6.2 Field Debug 로그

목적:

- 고객 현장, 설치 지원, 장애 대응 중 확인
- 운영자가 제한된 기간 동안 켜는 진단 로그

특징:

- product보다 자세함
- 여전히 secret redaction 필수
- 일정 시간 또는 session 단위로 켜는 것을 권장
- 로그 증가량을 예측 가능하게 유지

포함:

- protocol message type
- agent heartbeat interval
- job dispatch decision
- selector match count
- repository latency
- retry/backoff 정보
- task state transition
- redacted command metadata

금지:

- secret 원문
- private key
- enrollment token 원문
- signed payload raw dump
- stdout/stderr 전체 자동 기록

예시:

```text
DEBUG dispatch_selected_agents job_id=... selector=role=web count=18
DEBUG websocket_message_received agent_id=... message_type=heartbeat
DEBUG store_query_slow operation=find_pending_jobs elapsed_ms=531
```

### 6.3 Development/Test 로그

목적:

- 로컬 개발과 테스트 중 내부 상태 확인
- 테스트 실패 원인 분석
- protocol, parser, state machine 디버깅

특징:

- 가장 자세함
- production build 기본값이 아니어야 한다.
- test fixture와 local sample data 기준으로 사용한다.
- 실제 customer secret이 들어갈 수 있는 환경에서는 사용하지 않는다.

포함 가능:

- parser intermediate state
- state machine transition detail
- mock repository calls
- test fixture payload
- local-only stack trace

주의:

- 개발 로그도 redaction path를 우회하지 않는다.
- 개발 편의를 위해 production code에 `println!`을 남기지 않는다.
- 테스트에서 로그 검증이 필요하면 `tracing` subscriber를 test 전용으로 구성한다.

### 6.4 로그 구현 규칙

Rust:

- `tracing`을 표준으로 사용한다.
- `println!`, `dbg!`, `eprintln!`은 production path에 남기지 않는다.
- log profile은 bootstrap에서 한 번 결정한다.
- redaction은 logger 바깥이 아니라 structured field 생성 전 또는 field formatter에서 적용한다.

권장 enum:

```rust
pub enum LogProfile {
    Product,
    FieldDebug,
    Development,
}
```

금지:

- runtime 중 env var로 log level 변경
- request parameter로 global log level 변경
- secret redaction 없이 `?payload` dump
- command output을 info log에 자동 기록

허용:

- controller restart 후 log profile 변경
- scoped diagnostic session을 domain/audit와 함께 명시적으로 생성
- 특정 job output은 job log storage에 저장하되 일반 application log와 분리

## 7. 테스트 전략

### 7.1 테스트 피라미드

우선순위:

1. Domain unit test
2. Application use case test
3. Repository contract test
4. Protocol compatibility test
5. Controller/Agent integration test
6. CLI smoke test
7. Web Admin UI component/smoke test

가장 많이 작성해야 하는 테스트는 domain/application test다.

### 7.2 Domain 테스트

대상:

- job state transition
- agent state transition
- selector match
- drift diff
- approval rule
- redaction rule
- settings validation

특징:

- DB 없음
- network 없음
- filesystem 없음
- deterministic clock/random 사용

### 7.3 Application 테스트

대상:

- enroll agent
- create job
- dispatch job
- receive output
- complete job
- write audit
- check drift

규칙:

- repository는 trait mock/fake 사용
- clock/id generator는 fake 사용
- env var 사용 금지

### 7.4 Infrastructure 테스트

대상:

- SQLite/Postgres repository
- filesystem artifact store
- process runner
- systemd adapter
- log tail adapter

규칙:

- 외부 의존이 있으면 `#[ignore]` 또는 feature flag로 분리
- destructive command 금지
- root 권한 요구 테스트는 기본 test suite에 넣지 않는다.

### 7.5 Protocol 테스트

대상:

- agent enrollment messages
- heartbeat
- task assignment
- stdout/stderr chunks
- task result
- drift report

필수:

- backward compatibility fixture
- unknown field handling
- invalid signature handling
- malformed payload rejection

### 7.6 Web Admin UI 테스트

UI는 얇게 유지한다.

필수:

- API client type check
- agents list rendering
- job live output rendering
- dangerous action confirmation
- drift diff rendering
- audit list rendering

금지:

- UI에 domain rule 중복 구현
- UI state로 authorization 결정
- UI에서 secret 원문 표시

## 8. 보안 개발 규칙

### 8.1 기본 보안 자세

Sponzey Fleet는 원격 명령 실행 플랫폼이다. 모든 기능은 잠재적 권한 상승 경로로 본다.

필수:

- enrollment token은 생성 시 1회만 노출
- token 저장 시 hash 또는 안전한 secret storage 사용
- agent identity는 key pair 기반으로 설계
- controller identity는 key pair 기반으로 설계
- enrollment 이후 agent는 controller public key를 pinning한다.
- WebSocket task channel은 agent identity proof 이후에만 열린다.
- task payload는 controller-signed envelope로만 전달한다.
- agent는 unsigned, invalid signature, expired, replayed, target mismatch task를 실행하지 않는다.
- audit log는 append-only 모델을 기본으로 한다.
- secret redaction은 application log와 job output 모두에 적용한다.
- 제품, 고객, 운영, 공동 사용, 장시간 실행 환경의 agent-controller 통신은 반드시 HTTPS/TLS를 사용한다.
- HTTP controller URL도 기술적으로는 허용하지만 테스트 전용 transport로만 취급한다.
- HTTP를 사용할 때마다 명확한 경고 메시지를 출력해야 한다.
- HTTP controller URL이 controller external URL로 설정되면 Security audit에 `insecure_http_transport_enabled` 이벤트를 남긴다.
- HTTP transport는 기밀성/무결성 보장을 제공하지 않으며 token 노출, command 탈취, 데이터 유출, 중간자 공격 위험이 있음을 문서화한다.

HTTP 허용 정책:

- HTTP를 허용하기 위해 별도 예외 옵션을 두지 않는다.
- `http://127.0.0.1`, `http://localhost`, `http://192.168...`, 내부망 hostname 모두 URL 형식이 유효하면 허용하지만 테스트 용도로만 안내한다.
- 단, `0.0.0.0` 또는 `::` 같은 wildcard bind host를 agent/controller URL로 쓰는 것은 거부한다.
- HTTPS를 기본 제품 경로로 문서화하되, HTTP 사용 방법은 테스트 전용이라는 경고와 함께만 안내한다.

### 8.2 Root 권한 실행

Agent는 root로 실행될 수 있다. 따라서 실행 boundary를 엄격히 둔다.

필수:

- task timeout
- allowed primitive
- dangerous task classification
- approval requirement
- high-risk explicit confirmation
- working directory 제한
- output size limit
- process kill on cancel/timeout

금지:

- controller에서 받은 문자열을 shell에 그대로 넘기는 기본 구현
- command allowlist 없는 high-risk action
- high-risk command를 confirmation 없이 실행
- `/` 기준 recursive change를 쉽게 허용
- secret을 command argument로 노출하는 API 디자인

### 8.3 Secret 처리

원칙:

- secret은 가능한 한 저장하지 않는다.
- 저장해야 하면 암호화한다.
- 출력해야 하면 redact한다.
- audit에는 secret reference만 남긴다.

금지:

- token을 URL query string으로 전달
- secret을 일반 log field에 기록
- secret 포함 payload를 debug dump
- `.env` 파일을 운영 핵심 설정 수단으로 의존

## 9. API와 Protocol 규칙

### 9.1 API

API handler는 얇게 유지한다.

역할:

- request parse
- authentication
- authorization check
- DTO validation
- use case 호출
- response mapping

금지:

- DB 직접 접근
- domain state 직접 조작
- process env 읽기
- shell command 실행
- audit 누락

### 9.2 Protocol

Agent-controller protocol은 명시적 version을 가져야 한다.

필수:

- protocol version
- message id
- correlation id
- agent id
- timestamp
- message type
- payload schema version

오류 처리:

- unknown message는 reject 또는 ignore 정책을 명확히 둔다.
- malformed payload는 audit 가능한 security event로 남긴다.
- 재시도 가능한 오류와 치명 오류를 구분한다.

### 9.3 Persistent Agent Session

Sponzey Fleet의 원격 실행 경로는 Agent가 Controller에 outbound WebSocket session을 유지하는 구조를 기본으로 한다.

목표:

- Controller가 Agent로 직접 TCP 접속하지 않는다.
- Agent가 Controller에 인증된 persistent session을 열어 둔다.
- Controller는 이미 인증된 Agent session으로 task를 즉시 push한다.
- heartbeat는 연결을 여는 주기가 아니라 session liveness signal이다.
- facts, metrics, operational log 전송 주기는 task dispatch 즉시성과 분리한다.

필수 규칙:

- WebSocket task channel은 agent identity proof 이후에만 session registry에 등록한다.
- active session registry는 runtime infrastructure state다. WebSocket handle이나 channel sender를 DB/domain object에 저장하지 않는다.
- duplicate session은 명확한 정책을 가진다. 기본 방향은 new session wins이며, 기존 session close reason과 audit를 남긴다.
- revoked/disabled agent의 active session은 revoke API 성공 직후 닫는다.
- connected agent에 job을 dispatch할 때도 controller-signed task envelope, approval, expiry, nonce replay, target 검증을 우회하지 않는다.
- DB에 job/assignment를 저장하기 전에 WebSocket으로 task를 먼저 보내지 않는다.
- store lock을 잡은 상태에서 WebSocket read/write await를 수행하지 않는다.
- socket writer는 session당 하나만 둔다. heartbeat, facts, metrics, log, output producer는 outbound queue로 message를 전달한다.
- command/runbook 실행 중에도 heartbeat/liveness와 session read/write가 막히지 않아야 한다.
- output chunk는 job output storage에 저장하고 Product application log에는 원문을 남기지 않는다.

권장 구현 경계:

```text
Controller HTTP API
  -> Application dispatch use case
  -> Store repository
  -> SessionRegistry/SessionDispatcher
  -> WebSocket writer loop

Agent session
  -> read loop
  -> single writer queue
  -> heartbeat/facts/metrics/log ticks
  -> task worker
```

거부해야 하는 패턴:

- heartbeat 주기마다 연결을 열고 닫는 구조를 제품 기본 경로로 유지하는 것
- 즉시성을 이유로 Controller가 Agent로 inbound 접속하는 구조
- 여러 thread/task가 같은 WebSocket writer에 직접 쓰는 구조
- active session이 있다는 이유로 high-risk approval을 생략하는 구조
- UI가 agent connected/running 상태를 자체 추정하는 구조

## 10. Web Admin UI 규칙

Web Admin UI는 얇은 운영 표면이다.

해야 할 것:

- agent 상태 확인
- job 실행
- live output 확인
- facts/metrics snapshot 확인
- drift diff 확인
- approval 처리
- audit 조회

하지 말 것:

- 복잡한 workflow designer
- 무거운 dashboard builder
- 설정 파일 편집기
- runtime env editor
- domain rule 재구현
- 별도 Node.js web server 운영

UI 기술:

- React/Vite 또는 SvelteKit static export
- TypeScript
- generated API client
- CSS는 단순하고 유지 가능한 방식
- controller가 static asset으로 서빙

권한:

- UI는 authorization을 결정하지 않는다.
- 모든 권한 판단은 controller application layer에서 한다.
- UI는 forbidden response를 명확히 보여주는 역할만 한다.

## 11. 코드 스타일

### 11.1 Rust

원칙:

- `cargo fmt` 기준을 따른다.
- `cargo clippy` 경고를 무시하지 않는다.
- error는 `thiserror` 또는 명확한 enum으로 모델링한다.
- binary entrypoint는 작게 유지한다.
- `unwrap`, `expect`는 테스트 또는 bootstrap fatal path에서만 제한적으로 사용한다.
- domain error와 infrastructure error를 구분한다.

금지:

- `anyhow::Result`를 domain layer public API에 노출
- global mutable state
- runtime env lookup
- blocking IO를 async executor에서 무분별하게 실행
- production path의 `println!`/`dbg!`

권장:

- typed id newtype
- explicit state enum
- small module
- trait boundary
- `tracing` span
- deterministic tests

### 11.2 TypeScript/Web

원칙:

- UI는 얇게 유지한다.
- generated API type을 사용한다.
- domain rule을 복제하지 않는다.
- secret 표시를 기본 금지한다.
- dangerous action에는 명확한 confirmation을 둔다.

금지:

- 전역 mutable config store
- 브라우저 localStorage에 token 장기 저장
- UI에서 env/runtime 설정 변경
- API 에러 무시

## 12. 변경 절차

### 12.1 새 기능

순서:

1. 요구사항을 `PROJECT.md`와 비교한다.
2. domain/application 영향 범위를 정한다.
3. 실패하는 테스트를 작성한다.
4. 최소 구현한다.
5. logging/audit/security 영향을 확인한다.
6. CLI/API/UI 노출이 필요하면 얇게 연결한다.
7. 문서를 갱신한다.

### 12.2 버그 수정

순서:

1. 재현 테스트 작성
2. 원인 위치 확인
3. 최소 수정
4. regression test 유지
5. 로그 또는 audit 누락 여부 확인

### 12.3 리팩터링

순서:

1. behavior 보존 테스트 확인
2. 작은 단위로 정리
3. 이름/경계/의존성 개선
4. 기능 변경과 분리

## 13. 거부해야 하는 요청

다음 요청은 프로젝트 규칙 위반으로 거부하거나 대안을 제시한다.

- 실행 중인 process env를 바꿔 설정을 변경하자는 요청
- request handler에서 env var를 읽자는 요청
- UI에서 controller runtime config를 직접 patch하자는 요청
- domain layer에서 DB나 filesystem을 직접 쓰자는 요청
- 테스트 없이 enrollment/token/task signature를 구현하자는 요청
- controller 서명 없는 task를 agent가 실행하게 하자는 요청
- 제품/운영 용도에서 HTTP transport를 사용하거나 권장하자는 요청
- high-risk command를 confirmation 없이 실행하자는 요청
- production log에 command output 전체를 남기자는 요청
- secret redaction 없이 debug dump를 남기자는 요청
- Ansible full compatibility를 MVP로 넣자는 요청
- Web Admin UI를 무거운 standalone web platform으로 키우자는 요청
- root shell execution을 기본 primitive로 무제한 허용하자는 요청

대안:

- 설정 변경은 explicit config object와 restart/reload command로 설계한다.
- 위험 명령은 approval과 audit를 거친다.
- domain rule은 Rust application/domain layer에 둔다.
- UI는 API를 호출하고 결과를 보여준다.

## 14. 완료 기준

작업 완료 전 확인한다.

- Layered/Clean Architecture 의존 방향을 지켰는가
- Tidy 작업과 behavior 변경을 구분했는가
- 핵심 로직 테스트가 있는가
- env var를 중간에 읽거나 바꾸지 않았는가
- 외부 설정 파일을 불필요하게 늘리지 않았는가
- log profile 3단계 원칙에 맞는가
- secret redaction이 적용되는가
- HTTP transport를 테스트 전용으로만 안내하고, 사용 시 경고/audit가 남는가
- controller public key pinning이 동작하는가
- authenticated agent만 task channel을 사용하는가
- controller-signed task envelope를 검증하는가
- unsigned/invalid/expired/replayed/target mismatch task를 거부하는가
- high-risk command confirmation을 요구하는가
- audit가 필요한 이벤트에 audit가 남는가
- root 권한 실행 boundary가 명확한가
- Web Admin UI에 domain rule이 중복되지 않았는가
- 문서가 필요한 변경이면 문서를 갱신했는가

## 15. 프로젝트 기본 명령 후보

실제 구현 후 명령은 달라질 수 있지만, 작업자는 다음 방향을 기준으로 설계한다.

```bash
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace

npm install
npm run build --workspace web-admin
npm test --workspace web-admin
```

npm은 Rust core 런타임이 아니라 Web Admin UI 빌드와 바이너리 배포 wrapper에 사용한다.

## 16. 최종 원칙

Sponzey Fleet는 운영 자동화 제품이다. 빠른 데모보다 중요한 것은 신뢰 가능한 실행 경계, 감사 가능성, 설정의 명시성, 테스트 가능한 구조다.

개발자는 다음 판단 기준을 계속 사용한다.

- 이 코드는 테스트 가능한가
- 이 설정은 언제, 어디서, 한 번만 결정되는가
- 이 로그는 누구를 위한 로그인가
- 이 기능은 domain/application/infrastructure 중 어디에 속하는가
- 이 실행은 audit와 approval이 필요한가
- 이 UI는 얇은 운영 표면인가, 아니면 business rule을 복제하고 있는가

이 질문에 명확히 답하지 못하면 구현을 멈추고 구조를 먼저 정리한다.
