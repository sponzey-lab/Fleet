# Sponzey Fleet Development Rules

이 문서는 Sponzey Fleet 저장소에서 코드, 테스트, 문서, schema, 배포 구성을 변경하는
사람과 자동화 에이전트가 따라야 하는 강제 규칙이다. 제품 방향은 `PROJECT.md`, 현재
지원 상태는 실제 코드·test·manifest와 `docs/feature-matrix.md`를 함께 근거로 판단한다.

## 1. Project Context and Method Selection

### 1.1 제품과 지배적 위험

Sponzey Fleet는 agent가 Controller에 outbound persistent WebSocket session을 유지하고,
Controller가 인증·승인·서명된 작업을 agent에 전달하는 서버 운영 자동화 플랫폼이다.
제품은 root 권한 실행, credential, 원격 명령, 상태 수집, drift detection, remediation,
artifact, audit를 다룬다. 따라서 다음 위험은 일반 기능 편의보다 우선한다.

- 잘못된 대상 또는 승인되지 않은 명령 실행
- agent/controller identity, token, signing key, TLS material 또는 secret 노출
- duplicate, stale, replayed message로 인한 상태 오염이나 중복 실행
- migration, retention, backup/restore로 인한 데이터 손실
- WebSocket runtime state와 durable state의 혼동
- API, protocol, persistence, Web Admin, npm package 사이의 compatibility 불일치
- shutdown, cancel, timeout이 없는 background 또는 child-process 작업

### 1.2 근거 우선순위

작업 전에 아래 순서로 근거를 확인한다.

1. system/developer/user의 현재 명시적 지시와 가장 가까운 `AGENTS.md`
2. compile되는 source, 실행되는 test, manifest, migration과 public contract snapshot
3. `docs/feature-matrix.md`, `docs/security.md`, `docs/protocol.md`, `docs/storage.md`,
   `docs/api.md`, `docs/release-gate.md`
4. `README.md`와 `README.ko.md`의 사용자 흐름
5. `PROJECT.md`의 제품 목표와 장기 계획
6. `.tasks/`의 과거 phase·task 기록

하위 근거가 상위 근거와 충돌하면 현재 구현으로 가장하거나 임의로 제품 범위를 바꾸지
않는다. public contract, data ownership 또는 제품 범위를 바꾸는 충돌은 사용자에게
보고하고 별도 결정으로 남긴다. `.tasks/`에 root `plan.md`가 없거나 과거 phase만 있을
수 있으므로, 문서가 참조하는 계획 파일의 존재와 현재성을 작업마다 다시 확인한다.

### 1.3 확인된 현재 구현과 목표 구조

작업 시작 시 manifest와 source로 다시 확인하되, 이 규칙을 정리한 시점의 기준선은
다음과 같다.

- Rust 2024 edition workspace이며 제품 binary는 `fleet-cli`가 만드는 `fleet` 하나다.
- `fleet-domain`은 project dependency와 external dependency가 없는 순수 Rust crate다.
- `fleet-application`은 domain port와 use case를 소유한다.
- `fleet-protocol`은 Serde JSON wire DTO와 protocol version boundary를 소유한다.
- `fleet-store`는 SQLite 기본 store와 feature-gated Postgres adapter를 소유한다.
- `fleet-runner`는 process, package, service, file, template 같은 host side effect를
  소유한다.
- `fleet-controller`는 Axum HTTP/WebSocket interface, static asset serving, controller
  runtime composition과 worker를 포함한다.
- `fleet-cli`는 clap entrypoint와 composition root이며, 현재 agent session/runtime와
  일부 local operations도 포함한다.
- `fleet-agent`는 현재 agent library의 목표 경계만 가진 얇은 crate다. agent 기능이 이미
  이 crate에 있다고 가정하지 않는다.
- Web Admin은 현재 React/Vite가 아니다. ES module 기반 정적 JavaScript, CSS, HTML,
  dependency-free API client, `allowJs + checkJs` 검증과 Node test/build script를 쓴다.
- npm은 Rust runtime이 아니라 `@sponzey/fleet` wrapper와 Darwin/Linux arm64/x64
  platform binary package 배포에만 사용한다.
- SQLite가 기본 production-ready store다. Postgres는 feature-gated partial path이며,
  S3 adapter, full mTLS client-certificate enforcement, OIDC/project RBAC, HA coordination,
  Windows service와 automatic self-update는 현재 완료된 기능으로 주장하지 않는다.

`PROJECT.md`가 설명하는 `fleet-agent` runtime 분리, 더 얇은 interface, Postgres 운영
성숙도, 외부 secret/artifact adapter와 cross-platform 지원은 목표 구조다. 목표 구조로
이동할 때 characterization test와 작은 Tidy First 단계를 사용하며, 존재하지 않는
구조를 현재 구현처럼 문서화하지 않는다.

### 1.4 작업별 방법론 선택

각 작업 시작 시 작업 기록이나 첫 진행 보고에 아래 판단을 짧게 남긴다.

```text
현재 순간:
대상 언어·실행 단위:
지배적 위험:
선택한 방법론·패턴:
검증 근거·종료 기준:
```

- 작고 가역적인 변경과 명확한 최초 기능은 짧은 TDD 수직 기능으로 수행한다.
- 버그는 재현 test, 최소 수정, regression test 순서로 처리한다.
- 기존 구조가 변경을 방해할 때만 behavior-preserving Tidy First를 먼저 분리한다.
- 테스트가 없는 legacy path는 characterization test로 현재 동작을 고정한 뒤 바꾼다.
- 사용자 가치나 기술 가능성이 실제로 불명확할 때만 time-boxed prototype을 쓴다.
- 폐기형 prototype은 production path, 실제 credential, 사용자 데이터와 비가역 side
  effect에서 격리하고 production 코드로 승격하지 않는다.
- migration, security, protocol, public API, release, credential과 비가역 변경은 대안,
  rehearsal, compatibility, rollback과 release gate가 있는 완전한 위험 주기로 수행한다.
- 새 근거로 위험이나 변경 순간이 바뀌면 방법론과 검증 강도를 다시 선택한다.

## 2. Architecture and Dependency Rules

### 2.1 논리 계층

책임을 다음 계층으로 구분하고 dependency는 내부로 향하게 한다.

```text
Presentation / Interface
  clap CLI, Axum HTTP/WebSocket, static Web Admin
            |
            v
Application
  use case, port, orchestration, authorization/dispatch flow
            |
            v
Domain
  entity, value object, policy, invariant, state transition, domain error

Infrastructure -> Application/Domain ports
  SQLite/Postgres, filesystem artifact store, process runner, TLS/network,
  clock/random/id, service manager

External Interface
  JSON wire protocol, OpenAPI/API schema, npm/release artifact contract
```

- Domain은 DB, HTTP, WebSocket, filesystem, environment, process runner와 framework를
  직접 사용하지 않는다.
- Application은 concrete store, network client, filesystem, process environment와 UI
  type을 직접 사용하지 않는다. 필요한 I/O는 application이 소유하는 port로 표현한다.
- Interface는 parse, authentication, authorization, DTO validation, use case 호출과
  response mapping만 수행한다. handler에 SQL, shell execution, state transition 규칙을
  넣지 않는다.
- Infrastructure는 내부 port를 구현하고 composition root에서 선택한다. infrastructure
  error는 SQL, URL credential, key path 또는 raw payload를 외부로 그대로 누출하지 않는다.
- wire DTO, HTTP DTO, persistence record, domain object와 Web Admin view model을 하나의
  type으로 재사용하지 않는다. 각 boundary에서 명시적으로 변환한다.

### 2.2 현재 crate dependency 계약

`Cargo.toml`과 `cargo metadata --no-deps`로 다음 방향을 유지한다.

```text
fleet-domain       -> no project crate, no external runtime/framework dependency
fleet-core         -> no project crate; bootstrap/settings/identity/logging support
fleet-application  -> fleet-domain
fleet-protocol     -> fleet-domain + serialization only
fleet-store        -> fleet-application + fleet-domain
fleet-runner       -> fleet-domain
fleet-controller   -> fleet-core/application/domain/protocol/store
fleet-agent        -> agent-specific library boundary; no independent binary
fleet-cli          -> composition root and the only shipped binary
```

- `fleet-domain`에 `tokio`, Axum, database client, Serde wire schema, filesystem adapter,
  `tracing-subscriber`를 추가하지 않는다.
- `fleet-application`에서 `std::env`, SQL, Axum, reqwest, tungstenite 또는 concrete store를
  사용하지 않는다.
- `fleet-protocol`은 transport serialization을 소유하지만 domain behavior를 복제하지
  않는다. domain type 공유는 의도적인 value boundary에만 허용한다.
- `fleet-store`와 `fleet-runner`는 서로를 import하지 않는다.
- `fleet-controller` 또는 `fleet-cli`의 내부 DTO를 protocol source of truth로 사용하지
  않는다.
- `fleet-controller`와 `fleet-agent`에 `main.rs`를 만들지 않는다. binary와 service unit,
  npm wrapper, release archive는 모두 resolved `fleet` binary를 사용한다.
- `fleet-cli`가 현재 agent runtime을 포함한다는 사실은 migration 대상이지 새 domain
  rule을 계속 넣을 허가가 아니다. agent-specific runtime을 `fleet-agent`로 옮길 때는
  behavior test를 먼저 고정하고 작은 이동을 별도 tidy 단위로 수행한다.

### 2.3 state와 side-effect owner

- canonical persistent state의 write owner는 repository를 호출하는 application use case다.
  HTTP handler, Web Admin, protocol decoder는 직접 write owner가 되지 않는다.
- active WebSocket handle, channel sender, connection ack와 session liveness는 controller
  runtime의 `SessionRegistry` 계열 state다. DB/domain object에 저장하지 않는다.
- Job과 Assignment는 별도 canonical state다. Job aggregate가 Assignment terminal 상태를
  덮어쓰지 않으며, target snapshot 이후 label 변화로 기존 target을 다시 계산하지 않는다.
- output chunk, final result, Product Log, agent operational log, audit event와 rendered
  artifact를 서로 다른 저장·retention 경계로 유지한다.
- 한 canonical state에는 하나의 write owner만 둔다. 다른 component는 command 또는
  typed port로 변경을 요청한다.

### 2.4 component 분리 기준

작은 module은 함께 둘 수 있다. 다음 중 하나가 생길 때만 물리적으로 분리한다.

- 독립 변경 이유와 public contract가 있다.
- 외부 I/O 또는 security boundary가 다르다.
- 별도 lifecycle, cancellation 또는 resource owner가 있다.
- 독립 contract/integration test가 필요하다.

이 조건이 없으면 추측성 interface, repository, facade, wrapper 또는 새 crate를 만들지
않는다. 큰 파일이라는 이유만으로 대규모 이동하지 않고 변경 축과 검증 가능한 seam을
먼저 만든다.

## 3. Language and Design Pattern Rules

### 3.1 Rust

- workspace `edition = "2024"`와 stable Rust에서 유효한 관용구를 사용한다. 저장소에
  `rust-toolchain` pin이 없으므로 local compiler version을 repository requirement로
  임의 고정하지 않는다.
- 불변식과 식별자는 enum, newtype, private field와 validated constructor로 표현한다.
- 상태 전이는 enum과 명시적 method/pure reducer를 우선하고 GoF class 계층을 복제하지
  않는다.
- 오류는 분류 가능한 enum/struct로 만들고 `Display`와 `Error` contract를 제공한다.
  Domain public API에 `anyhow::Result`, raw SQL/network error 또는 문자열-only 상태를
  노출하지 않는다.
- `unwrap`과 `expect`는 test 또는 복구 불가능한 bootstrap invariant에서만 사용하고,
  bootstrap에서도 민감 정보가 panic text에 포함되지 않게 한다.
- async executor에서 blocking DB/process/filesystem 작업을 직접 오래 수행하지 않는다.
  현재 blocking Postgres pool과 process runner는 infrastructure boundary에 격리하고,
  async 호출자가 lock을 잡은 채 blocking 또는 network await를 하지 않게 한다.
- `unsafe`를 추가하면 `# Safety` contract, 최소 범위, invariant test와 대안 검토를 같은
  변경에 포함한다.
- formatter는 `cargo fmt --all --check`, lint는
  `cargo clippy --workspace --all-targets -- -D warnings`를 기준으로 한다.

### 3.2 Web Admin JavaScript

- 현재 Web Admin은 browser-native ES module과 static export다. 별도 승인된 architecture
  변경 없이 React, Vite, Svelte, bundler, runtime Node server 또는 무거운 state framework를
  추가하지 않는다.
- `web-admin/tsconfig.json`의 `allowJs`, `checkJs`, `strict`, `noEmit` contract를 유지한다.
- browser UI는 단방향 local view state와 `api-client.js` command/effect boundary를 쓴다.
  render 중 직접 persistence, credential discovery 또는 controller 설정 변경을 하지 않는다.
- API client type과 endpoint coverage는 `api.schema.json`, `docs/openapi.json`,
  `scripts/typecheck.js`, `scripts/test.js`로 함께 검증한다.
- admin token을 `localStorage`, 장기 browser storage, URL query 또는 rendered HTML에
  저장하지 않는다.

### 3.3 Shell, Node packaging과 workflow

- shell script는 기존 interpreter(`sh` 또는 명시된 `bash`)의 문법 범위를 지키고,
  `set -eu` 또는 `set -euo pipefail`을 해당 shell에 맞게 사용한다.
- script 입력, destructive side effect와 exit behavior가 자명하지 않으면 shebang 직후에
  설명한다. 경로와 인자는 quote하고 broad glob 또는 unresolved environment variable을
  destructive target으로 쓰지 않는다.
- npm/Node 코드는 Web Admin 검증과 binary distribution wrapper만 담당한다. Rust domain
  behavior나 controller runtime을 JavaScript로 복제하지 않는다.
- workflow expression, shell variable와 npm metadata 변경은 local check script로 검증한다.
  release workflow는 GitHub-hosted runner와 npm Trusted Publishing OIDC를 유지한다.

### 3.4 패턴 선택

- Repository는 aggregate persistence, backend 교체와 shared contract test가 실제로
  필요할 때만 쓴다. entity마다 repository를 만들지 않는다.
- Adapter는 SQLite/Postgres, local/remote artifact, service/package manager, HTTP/wire DTO
  변환 같은 실제 외부 경계에만 둔다.
- Command/use case는 approval, audit, retry, queue 또는 지연 실행이 필요한 operation에
  쓴다. 단순 동기 helper를 command object로 감싸지 않는다.
- State machine은 retry, cancel, resume, arbitration 또는 둘 이상의 async 단계가 결합된
  lifecycle에만 쓴다. 단순 parse/transform 흐름에는 만들지 않는다.
- Supervisor/worker owner는 long-running restart/shutdown 책임이 있을 때만 둔다.
- Strategy는 실제 교체되는 policy 축이 있을 때만 쓰고, 단일 함수나 closure로 충분하면
  추가 trait 계층을 만들지 않는다.
- process-local observer/stream은 일시 알림에만 쓴다. durable queue, replay source 또는
  canonical write 경로를 대신하지 않는다.

## 4. Code Documentation and Comments

### 4.1 `source.md` 탐색 인덱스

현재 저장소에는 `source.md`가 없다. 기존 누락을 한 번에 문서화하는 대규모 churn을 만들지
않고 다음 source 변경부터 touched boundary에 점진적으로 도입한다.

- 사람이 유지하는 production/test/tool/script source가 직접 3개 이상이거나, 독립된
  ownership·architecture boundary·entrypoint가 있는 의미 있는 source directory에
  `source.md`를 둔다.
- 위 조건을 만족하는 `crates/*/src`, `web-admin`, 독립 `web-admin/scripts`, `scripts`,
  `npm/fleet`과 `npm/fleet/scripts` 경계를 변경하면 가장 가까운 index를 생성하거나
  갱신한다. 작은 하위 directory는 가장 가까운 상위 index에 포함한다.
- index가 생긴 범위의 handwritten source는 가장 가까운 `source.md`에 정확히 한 번만
  등재한다. 자체 index가 있는 하위 경계는 상위에서 개별 file을 중복하지 않고 index
  link와 책임만 기록한다.
- generated, vendored, dependency, fixture/snapshot, binary asset와 build output(`target`,
  `web-admin/dist`, `dist`, platform binary)은 파일별 등재에서 제외한다. generator source,
  schema 또는 handwritten wrapper만 등재한다.
- 형식은 `Path | Kind | Responsibility | Boundary / Side effects`를 사용한다. Kind는
  `Domain`, `Application`, `Protocol`, `Infrastructure`, `Interface`, `UI`, `Tooling`,
  `Packaging`, `Test` 중 실제 책임에 맞는 값을 쓴다.
- symbol/signature 목록, 알고리즘, 구현 절차, 진행 상태, 작성자, 변경 이력을 넣지 않는다.
- `source.md`는 “어디에 무엇이 있는가”만 답하는 비권위 탐색 cache다. 코드, manifest,
  test, schema와 승인된 public contract를 대신하지 않는다. index로 대상을 찾은 후 관련
  source와 test를 직접 읽고 수정한다.

Source 추가·삭제·이동 또는 책임·계층·state owner·중요 side effect가 바뀌면 같은
변경에서 index를 갱신한다. private helper 추출, symbol rename 또는 formatting처럼 file
책임이 유지되면 index를 바꾸지 않는다. 완료 전에 path, link, 누락, 중복과 실제 책임을
확인한다.

### 4.2 file/module와 declaration 문서

- `source.md`는 위치, file/module header는 경계가 존재하는 이유, declaration 문서는
  호출자 contract만 소유한다. 같은 설명을 반복하지 않는다.
- Rust public module 또는 path만으로 ownership·invariant·I/O가 명확하지 않은 module은
  파일 시작 `//!` rustdoc을 쓴다. public API, port/adapter boundary, security/state/
  concurrency contract와 비자명한 algorithm은 `///`를 쓴다.
- Rustdoc은 signature를 번역하지 않는다. 필요할 때만 domain parameter 의미, `# Returns`,
  `# Errors`, `# Panics`, `# Safety`를 기록한다.
- JavaScript public module 또는 복잡한 boundary는 `/** @file ... */`, exported function은
  JSDoc을 사용한다. TypeScript type을 주석에서 반복하지 않고 입력 의미, side effect와
  failure를 설명한다.
- Shell function은 인자, stdout/stderr, exit status 또는 destructive side effect가
  이름만으로 명확하지 않을 때 인접 `#` 주석을 쓴다.
- 자명한 private helper, getter, test fixture, 짧은 adapter glue와 모든 named symbol에
  문서를 강제하지 않는다. “무엇을 하는지”를 코드를 그대로 되풀이하는 주석은 삭제한다.
- credential, 실제 사용자 데이터, raw token/key를 example에 넣지 않는다. 변경 이력은
  주석이 아니라 version control에 둔다.

### 4.3 contract 문서 동기화

- REST route, request/response, permission 또는 error contract 변경은
  `docs/api.md`, `docs/openapi.json`, 필요 시 `web-admin/api.schema.json`, API client와
  coverage test를 같은 변경에서 갱신한다.
- `docs/openapi.json`과 `web-admin/api.schema.json`은 현재 test로 검증되는 handwritten
  contract snapshot이다. generator가 도입되기 전까지 서로 자동 생성물이라고 가정하지
  않는다. generator 도입 후에는 generator source를 수정하고 output을 재생성한다.
- wire message, version, lifecycle 또는 compatibility 변경은 `fleet-protocol` test와
  `docs/protocol.md`를 함께 갱신한다.
- schema, repository, migration, retention 또는 backup contract 변경은
  `docs/storage.md`와 migration fixture/gate를 함께 갱신한다.
- security, trust, auth, secret, approval 또는 audit boundary 변경은 `docs/security.md`와
  `docs/security-checklist.md`를 갱신한다.
- 사용자 command, 설치, warning 또는 workflow가 바뀌면 영어 `README.md`와 한국어
  `README.ko.md`를 같은 변경에서 동기화한다.
- 지원 상태가 바뀌면 `docs/feature-matrix.md`와 해당 release note의 current/partial/planned
  표현을 실제 구현과 맞춘다.

## 5. Configuration, Security, and Runtime

### 5.1 bootstrap-only configuration

- CLI argument, config file와 허용된 process environment는 process bootstrap에서 한 번
  읽고 typed settings로 parse·validate한 뒤 immutable value로 명시적으로 전달한다.
- Application/Domain, request handler, task execution 중간, Web Admin action과 background
  worker에서 process environment 또는 config file을 다시 읽어 동작을 바꾸지 않는다.
- `std::env::set_var`, `remove_var`, mutable global config, service locator, replaceable
  singleton과 runtime config patch endpoint를 금지한다.
- `std::env::consts`, `temp_dir`, `current_exe` 같은 platform/process 정보는 설정 재조회가
  아니지만, 이 값으로 domain policy를 숨기지 않는다.
- ignored Postgres integration test의 `FLEET_TEST_POSTGRES_URL`처럼 외부 integration
  입력이 필요한 경우 test bootstrap에서만 한 번 읽고 production path와 분리한다.
- child process별 환경이 필요하면 `Command` builder에 명시적으로 전달한다. parent process
  environment를 변경하지 않는다.
- npm postinstall/test script의 `process.env`는 해당 installer/test process의 입력
  boundary로만 사용한다. Rust runtime 설정이나 숨은 product feature toggle로 확장하지
  않는다.

외부 설정 파일은 controller data/identity, agent identity/config, CLI credential profile,
migration/fixture처럼 lifecycle과 owner가 명확한 경우에만 둔다. UI는 runtime 설정
editor가 아니며 controller/agent config를 직접 수정하지 않는다.

### 5.2 secret, identity와 transport

- raw admin/enrollment token은 생성 시 한 번만 표시하고 hash만 저장한다. URL query,
  Product/Field Log, audit, API response history, Web Admin state에 남기지 않는다.
- secret은 typed `SecretRef` 또는 secure provider boundary로 전달한다. raw secret을 일반
  file/DB/event에 저장하지 않고 `Display`/`Debug`/error에서 redact한다.
- TLS server identity, controller Ed25519 signing identity, agent Ed25519 identity와 future
  agent client certificate trust를 분리한다. fingerprint, key path와 certificate material을
  서로 대체하지 않는다.
- Agent는 signed task의 target, expiry, signing trust, nonce replay를 검증하고 하나라도
  실패하면 실행하지 않는다. unsigned, invalid, expired, replayed, target-mismatch task는
  typed rejection과 Security audit로 처리한다.
- Agent task channel은 agent identity proof가 끝난 후에만 연다. enrollment token을
  heartbeat/task channel credential로 재사용하지 않는다.
- Controller task delivery 전에 Job/Assignment와 signed envelope를 durable store에 먼저
  저장한다. active socket write 성공은 accepted 또는 started가 아니다.

### 5.3 HTTP/TLS 정책

- `http://` controller URL은 loopback, LAN, internal hostname을 포함해 기술적으로 허용하지만
  setup check, local/lab test와 short-lived validation 전용이다.
- product, customer, production, shared 또는 long-running 환경은 HTTPS/WSS를 사용한다.
- HTTP 사용마다 명확한 insecure warning을 출력하고, Controller external URL이 HTTP이면
  `insecure_http_transport_enabled` Security audit를 남긴다.
- HTTP 허용을 위한 숨은 exception flag를 만들지 않는다. `0.0.0.0`과 `::`는 bind host로는
  사용할 수 있어도 agent/controller external URL target으로는 거부한다.
- `--agent-client-ca-cert`는 listener enforcement가 완성되기 전 fail-closed로 거부한다.
  public certificate lifecycle metadata foundation을 full mTLS 지원이라고 문서화하지 않는다.

### 5.4 root task와 authorization

- root 또는 privileged task에는 allowed primitive, risk classification, approval, timeout,
  cancel/kill, working-directory/path boundary, output limit와 audit를 둔다.
- program과 args를 구조화해 process runner에 전달한다. Controller에서 받은 raw string을
  기본 shell command로 연결하거나 recursive `/` 변경을 쉽게 허용하지 않는다.
- high-risk confirmation flag는 compatibility acknowledgement일 뿐 approval을 대체하지
  않는다. Controller application의 approval state가 dispatch authority다.
- authenticated admin context가 actor와 permission을 결정한다. UI/request body의 actor,
  role, confirmation text를 authorization 근거로 신뢰하지 않는다.
- UI는 forbidden response를 표시할 수 있지만 권한, job state, agent state와 drift 결과를
  자체 추론하거나 canonical write하지 않는다.

### 5.5 persistence와 release security

- Audit는 API/Application 경계에서 append-only이며 일반 retention에서 제외한다. 현재
  SQLite audit를 tamper-proof WORM이라고 주장하지 않는다.
- destructive migration은 backup, previous-schema fixture, rehearsal, explicit operator
  confirmation과 rollback 없이 수행하지 않는다.
- npm release workflow는 GitHub Actions OIDC Trusted Publisher를 사용한다. long-lived
  `NPM_TOKEN`을 workflow에 다시 추가하거나 secret 값을 읽고 출력하려 하지 않는다.
- repository license, Cargo/npm package metadata와 배포 binary license는
  `AGPL-3.0-only`로 일치시킨다.

## 6. State, Concurrency, and Logging

### 6.1 state machine 규칙

다음처럼 retry, approval, cancellation, resume, rotation 또는 여러 async 단계가 결합된
lifecycle은 Domain state machine으로 관리한다.

- Job과 Assignment
- Approval과 remediation
- Agent certificate lifecycle
- Controller signing key rotation과 staged trust rollout
- 향후 signed update/rollback lifecycle

각 state machine은 state, event/operation, guard, effect, timestamp/sequence, failure와
terminal state를 정의한다. happy path뿐 아니라 invalid transition, duplicate, stale,
late result, cancel/timeout race, snapshot restore와 replay를 test한다. 단순 동기 parse,
formatting 또는 one-shot command에 state machine을 만들지 않는다.

### 6.2 WebSocket과 background concurrency

- Agent가 Controller에 outbound session을 열며 Controller는 Agent로 inbound 접속하지
  않는다. heartbeat는 connection open cycle이나 task dispatch cadence가 아니다.
- session당 WebSocket writer는 하나만 둔다. heartbeat, task, telemetry producer는 bounded
  outbound queue로 writer에 전달한다.
- store mutex/transaction/connection checkout을 잡은 상태에서 WebSocket read/write await를
  하지 않는다.
- duplicate session은 new session wins를 기본으로 하고 이전 session close reason과 audit를
  남긴다. revoked/disabled agent session은 revoke 성공 직후 닫는다.
- queue overflow, disconnect와 send failure가 durable Assignment를 잃게 하지 않는다.
  queued/dispatched claim과 release transition은 repository contract를 통과한다.
- output chunk와 result는 sequence/idempotency 규칙을 갖는다. 같은 key와 같은 body는
  duplicate로 허용할 수 있지만, 같은 key와 다른 body는 raw body 없이 security conflict로
  처리한다.
- background worker와 spawned task에는 owner, bounded interval/backoff, cancellation,
  progress/timeout 기준, error reporting과 graceful shutdown path를 둔다. owner 없는 detached
  task를 추가하지 않는다.
- scheduled drift, retention, signing staged rollout은 현재 single-controller runtime
  limitation을 유지한다. lease/leader election 없이 HA-safe라고 주장하지 않는다.
- agent command/runbook execution이 session read/write와 heartbeat를 막지 않게 하고,
  cancel/timeout 시 child process를 종료한 후 별도 terminal status를 보고한다.

### 6.3 세 가지 로그 profile

모든 application log는 다음 중 하나로 분류한다. 분류할 수 없으면 추가하지 않는다.

- `Product`: 기본값. 사용자 영향, lifecycle 시작/종료와 terminal result만 낮은 volume의
  structured field로 기록한다.
- `FieldDebug`: bootstrap에서 승인된 범위·기간·보존 정책 안에서 protocol type, retry,
  selector count, latency와 redacted transition detail을 기록한다.
- `Development`: local/test 전용의 상세 진단이다. production 기본 profile로 활성화하지
  않고 실제 customer secret 환경에서 사용하지 않는다.

Rust application log는 `tracing`을 사용한다. `println!`/`eprintln!`은 CLI의 명시적 사용자
출력, warning과 fatal result에만 허용하고 application event logging에 사용하지 않는다.
`dbg!`는 production path에 남기지 않는다.

어떤 profile에도 raw token/password/private key/certificate body, request body 전체,
environment dump, command stdout/stderr, rendered secret artifact body, private key path 또는
stack trace flood를 기록하지 않는다. job stdout/stderr는 job output storage, agent product-safe
operational event는 agent log storage에 둔다. 민감 field는 structured field를 만들기 전에
redact하며 raw error 문자열을 Product/Field Log에 그대로 전달하지 않는다.

## 7. TDD, Tidy First, and Delivery Workflow

### 7.1 production behavior 변경

1. 관련 `AGENTS.md`, `source.md`, source, test, public docs, current diff를 읽는다.
2. 작업별 방법론과 위험을 기록한다.
3. 예상한 이유로 실패하는 가장 작은 test를 작성하고 실제 실패를 확인한다.
4. test를 통과시키는 최소 production 구현을 한다.
5. 관련 unit/contract/integration test와 security/logging 영향을 확인한다.
6. behavior를 보존하며 이름, 중복과 boundary를 정리한다.
7. public contract, source index와 문서를 같은 변경에서 동기화한다.
8. formatter, lint, test, smoke와 diff gate를 위험에 비례해 실행한다.

문서-only, typo, 단순 사용자 문구 변경은 failing test를 먼저 만들 필요가 없다. 다만 link,
command, schema reference, 영어/한국어 동기화와 `git diff --check`를 검증한다. test 삭제,
assertion 약화 또는 fake success fallback으로 실패를 숨기지 않는다.

### 7.2 Tidy First

Tidy First는 모든 작업 앞에 자동으로 수행하는 단계가 아니다. 기존 구조가 필요한 변경을
방해할 때만 다음 behavior-preserving 정리를 먼저 분리한다.

- 이름 명확화, 작은 helper 추출, duplicate fixture 정리
- 실제 boundary에 맞춘 trait seam 또는 module 이동
- error type과 dependency 방향 정리
- `fleet-cli`의 agent runtime을 `fleet-agent`로 옮기기 위한 작은 characterization seam

tidy에 schema, protocol, public API, 사용자 behavior, unrelated formatting, 대규모 folder
이동 또는 미래를 위한 abstraction을 섞지 않는다. tidy와 feature를 같은 요청에서 수행해야
하면 diff section, task 또는 commit을 분리한다.

### 7.3 compatibility, migration과 성능

- public API, OpenAPI, protocol, persistence, serialization과 npm package 변경은 version
  compatibility, old/new 양쪽 contract test, migration과 rollback 근거를 포함한다.
- protocol unknown field/version/rejection policy를 명시하고 legacy fixture를 유지한다.
- SQLite schema 변경은 `CURRENT_SCHEMA_VERSION`, repeatable migration, previous-version
  fixture, legacy row 보존, backup newer-schema rejection을 검증한다.
- Postgres 변경은 `postgres` feature와 shared repository contract를 별도로 검증한다.
- destructive migration, key rotation, release tag와 package publish는 rehearsal과 rollback
  없이 실행하지 않는다.
- 성능 변경은 baseline, 측정 방법, representative load와 회귀 허용치를 먼저 정의한다.
  근거 없이 batch size, concurrency, cache, DB index, SQLite pragma를 바꾸지 않는다.

### 7.4 검증 명령 선택

가장 작은 관련 gate부터 실행하고 위험이 커질수록 넓힌다.

```bash
# Rust targeted/full
cargo test -p <crate> <test-name>
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo tree -p fleet-domain

# Postgres feature 변경
cargo test -p fleet-store --features postgres
cargo test -p fleet-controller --features postgres

# Web Admin
npm test --workspace web-admin
npm run typecheck --workspace web-admin
npm run build --workspace web-admin

# npm wrapper/release metadata
npm test --workspace @sponzey/fleet

# security/release
./scripts/hardening_audit.sh
./scripts/release_readiness_gate.sh
git diff --check
```

manual Linux/root, real Postgres, registry publish와 external account 검증은 자동 local gate와
구분해 보고한다. 실행하지 않은 command를 통과했다고 쓰지 않는다. 전체 release 기준은
`docs/release-gate.md`가 소유하며, 변경 범위에 해당하는 focused/manual gate도 적용한다.

### 7.5 release와 Git

- release version은 root `Cargo.toml`, `Cargo.lock`, root/npm package manifests와 platform
  optional dependency version을 함께 맞춘다.
- 다섯 npm package의 name, repository, `AGPL-3.0-only` license와 version을 검증한다.
- `.github/workflows/npm-release.yml`의 Trusted Publisher identity는 organization
  `sponzey-lab`, repository `Fleet`, workflow `npm-release.yml`과 일치해야 한다.
- publish job에만 `id-token: write`를 주고, 필요한 release upload 범위에만
  `contents: write`를 둔다.
- 이미 publish된 tag를 이동하거나 재사용하지 않는다. 수정은 새 version commit과 새 tag로
  release한다.
- 이 repository를 GitHub에 push/publish하기 전 remote가
  `sponzey-lab/Fleet`인지, active `gh` account가 `Leonard-Sponzey`인지 확인한다.
- commit, tag, push, publish, release 생성은 사용자가 명시적으로 요청했을 때만 수행한다.
  요청을 받으면 working tree, branch, remote, version gate와 registry 결과를 확인한다.

## 8. Code Review and Prohibited Patterns

### 8.1 Code Review Checklist

- 선택한 방법론과 패턴이 현재 순간, 위험, Rust/JavaScript/Shell 특성과 실행 단위에
  맞는가?
- 현재 구현과 목표 구조를 구분했고 존재하지 않는 기능을 완료로 주장하지 않았는가?
- 계층 책임, crate dependency 방향, canonical state owner와 side-effect boundary가
  유지되는가?
- DTO, domain, persistence, view model과 wire schema가 boundary에서 변환되는가?
- 설정·secret·log·event에 runtime env lookup, hidden global 또는 민감 정보가 없는가?
- task signature, target, expiry, replay, approval와 authorization guard를 우회하지 않는가?
- background/concurrent 작업의 bounded queue, cancellation, stale/duplicate event, retry와
  shutdown이 검증되는가?
- WebSocket write 전에 durable Job/Assignment가 저장되고 store lock이 await를 가로지르지
  않는가?
- public API/protocol/schema/persistence 변경에 compatibility, migration, rollback과 양쪽
  contract test가 있는가?
- index 적용 범위의 handwritten source가 가장 가까운 `source.md`에 정확히 한 번 등재되고
  path/link/책임/boundary가 실제 source와 맞는가?
- file/module/declaration 문서가 언어 관용구와 실제 contract를 따르며 `source.md` 또는
  signature를 반복하지 않는가?
- failing test를 구현 전에 실행했고 formatter, static analysis, 관련 test/smoke를 실제로
  실행했는가?
- 자동 검증과 외부 계정, registry, real DB, root/device/운영 환경의 수동 검증을 구분했는가?
- 영어/한국어 README와 current feature/security/protocol/storage 문서가 서로 일치하는가?

### 8.2 Prohibited Patterns

다음을 추가하거나 권장하지 않는다.

- 추측성 interface/abstraction, interface-per-class, 깊은 상속, pattern 이름만 위한 wrapper
- global mutable state, service locator, runtime-replaceable singleton, owner 없는 task
- 문자열 topic/dynamic payload EventBus로 durable state나 typed protocol을 대체하는 구조
- Domain/Application의 DB, filesystem, network, environment 또는 concrete client 접근
- HTTP handler/Web Admin의 직접 SQL, process execution, credential/config mutation
- UI가 authorization, Job/Assignment/Agent state 또는 drift policy를 재구현하는 구조
- 여러 producer가 같은 WebSocket writer를 직접 소유하거나 socket handle을 DB에 저장하는 구조
- DB write 전 task 전송, output chunk를 success로 해석, send success를 accepted로 해석하는 구조
- controller-signed envelope, approval, expiry, replay 또는 target validation 우회
- unrestricted shell, broad recursive root path, timeout/output limit/cancel 없는 privileged task
- raw secret/token/key/certificate/output/request/environment를 log, audit, API, UI에 dump하는 코드
- production failure를 fake data, seeded response, success-looking fallback으로 숨기는 코드
- runtime env/config patch, hidden YAML feature toggle, UI runtime settings editor
- source를 읽지 않고 stale `source.md`만 믿는 변경, 깨진 link·누락·중복 index
- 모든 file/symbol에 강제하는 상투적 주석, signature 반복, 코드와 어긋난 stale comment
- generated/build output 직접 수정, test 삭제, assertion 약화, unrelated formatting churn
- 근거 없는 concurrency/cache/index/pragma/retention 변경
- full mTLS, S3, HA, OIDC, Windows/auto-update를 foundation만으로 구현 완료라고 표시하는 문서
- npm release workflow의 long-lived `NPM_TOKEN`, published tag 이동 또는 version 불일치 publish

## 9. Required Agent Behavior and Decision Rules

- 작업 전에 이 문서, 더 가까운 `AGENTS.md`, 관련 `source.md`, source, test, public docs,
  manifest, current branch와 `git diff`를 읽는다.
- 검색은 `rg`와 `rg --files`를 우선하고, index로 찾은 target은 수정 전에 직접 읽는다.
- 사용자 변경과 unrelated dirty worktree를 되돌리거나 덮어쓰지 않는다. overlap을 피할 수
  없으면 중단하고 사용자에게 정확한 충돌을 보고한다.
- production behavior 변경은 failing test를 먼저 실행한다. 예상과 다른 이유로 실패하면
  구현 전에 test 또는 가정을 교정한다.
- source 추가·이동·삭제 또는 책임 변경 시 가장 가까운 `source.md`를 같은 변경에서
  생성·갱신한다. public/boundary contract 변경 시 언어 관용적인 documentation도 함께
  갱신한다.
- API, schema, protocol, 설정, 사용자 문구, permission, data ownership, migration,
  retention과 release contract 변경을 최종 보고에서 명시한다.
- secret이나 GitHub Actions secret 값을 읽어 공개하려 하지 않는다. secret 이름/설정
  존재 확인과 raw value 접근을 구분한다.
- destructive action은 정확한 target을 read-only로 확인하고 recovery/backup을 확보한다.
  workspace root, home 또는 unresolved broad path를 recursive delete target으로 사용하지 않는다.
- 현재 요청과 무관한 문제는 scope를 확대해 즉석 수정하지 않는다. 목표, 입력, 출력,
  검증과 완료 기준이 있는 follow-up으로 기록한다.
- 실행한 command와 과거 기록을 구분하고, 실행하지 않은 test·manual gate·external publish를
  완료로 보고하지 않는다.
- 요구가 제품 범위, public contract, canonical owner 또는 security posture를 바꾸며 근거로
  결정할 수 없으면 임의 선택하지 않고 선택지와 trade-off를 사용자에게 요청한다.
- 완료 시 변경 file, behavior/contract 영향, 실제 검증 결과, 남은 manual/external gate와
  알려진 limitation을 간결하게 보고한다.
