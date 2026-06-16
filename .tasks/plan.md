# Sponzey Fleet Phase 004+ 개발 계획

작성일: 2026-06-15
기준 문서: `PROJECT.md`, `AGENTS.md`, `README.md`, `README.ko.md`, `docs/`, `.tasks/phase001`, `.tasks/phase002`, `.tasks/phase003`
기준 구현: v0.0.14 릴리스 후보 현재 워킹트리

이 문서는 현재 구현된 Sponzey Fleet를 `PROJECT.md`의 장기 제품 방향과 비교하여, MVP 이후 어떤 개발이 필요한지 정리한 제품화 계획이다.

현재 루트의 과거 계획과 task 파일은 `.tasks/phase003/`로 이관했다. 새 루트 `.tasks/plan.md`는 다음 개발 구간의 기준 문서로 사용한다.

## 1. 현재 상태 요약

Sponzey Fleet는 초기 MVP를 넘어섰고, 이제는 "작동하는 데모"보다 "운영 가능한 제품의 핵심 골격"을 갖추는 단계다.

현재까지 구현된 핵심 축은 다음과 같다.

- 단일 Rust 바이너리 `sponzey`
- `sponzey controller ...`
- `sponzey agent ...`
- Rust workspace 기반 core/controller/agent/CLI 구조
- npm global install wrapper와 platform package
- Linux/macOS용 release artifact 흐름
- controller init/start
- agent init/start
- enrollment token 생성과 agent 등록
- admin token 기반 Web Admin 접근
- controller built-in HTTP/HTTPS server
- HTTP 사용 허용 및 강한 경고 로그/문구
- HTTPS/TLS 준비 경로
- controller external URL
- agent controller fingerprint pinning
- outbound agent connection 구조
- phase003 기준 persistent WebSocket session
- controller에서 연결된 agent로 즉시 task push
- heartbeat/facts/metrics/log interval 분리
- agent reconnect 지속 시도
- agent revoke 시 session 종료 및 UI 상태 반영
- facts/metrics/drift 저장 및 paging API
- agent system time과 controller stored time 표시
- Web Admin agent list/detail
- Web Admin run command/job output
- Web Admin facts/metrics/drift 표시
- metrics chart 기본 구현
- enrollment token 생성 UI
- OpenAPI/Swagger 문서 기반
- audit event 일부 기록
- high-risk command confirmation 일부 적용
- signed task envelope 기반 실행 경계
- command, drift, 제한된 runbook primitive 실행

즉, 현재 시스템은 "controller 하나에 여러 agent가 outbound로 붙고, Web Admin에서 agent 상태와 기본 작업을 확인하고 실행할 수 있는 형태"까지 도달했다.

하지만 `PROJECT.md`가 지향하는 제품은 단순 command runner가 아니다. 목표는 agent 기반 실시간 서버 운영 자동화 플랫폼이며, 다음 영역이 아직 제품 수준으로 충분하지 않다.

- 권한/승인/감사 체계
- job lifecycle과 multi-agent fanout
- runbook DSL과 idempotent primitive
- policy/drift/remediation 루프
- inventory targeting과 agent capability model
- Postgres/backup/retention 등 production controller 운영성
- 설치/업그레이드/서비스 운영 패키징
- API/문서/SDK 안정화

## 2. 구현 상태와 PROJECT.md 목표 비교

### 2.1 설치와 배포

현재 구현:

- npm global install wrapper가 있다.
- `@sponzey/fleet`와 platform package 구조가 있다.
- Linux x64 glibc 호환 문제를 한 차례 대응했다.
- GitHub Actions release/npm publish 흐름이 있다.
- `sponzey --help`, `sponzey --version` 경로가 있다.
- 로컬 개발용 scripts가 있다.

PROJECT.md 목표:

- npm one-command quick start
- npx demo mode
- standalone binary archive
- `.deb`
- `.rpm`
- Homebrew tap
- Docker image
- Windows package
- one-line install script
- checksum/signature verification
- version pinning
- dry-run install
- upgrade command
- stable/beta channel

남은 개발:

- [ ] Linux/macOS standalone archive를 npm 외부 release artifact로 공식화
- [ ] release artifact checksum과 signature를 제품 문서에 연결
- [ ] one-line install script 설계와 구현
- [ ] install script dry-run과 version pinning 지원
- [ ] `.deb` 패키지
- [ ] `.rpm` 패키지
- [ ] Homebrew tap
- [ ] Docker image
- [ ] Windows package 또는 Windows 미지원 상태 명시
- [ ] `sponzey upgrade` 설계
- [ ] stable/beta/nightly channel 정책
- [ ] npm global path 문제를 README troubleshooting에 더 명확히 반영

우선순위 판단:

- npm 설치 UX는 이미 핵심 흐름이 있으므로, 다음은 "운영 서버에 어떻게 안전하게 설치하고 업그레이드할 것인가"가 중요하다.
- Windows는 `PROJECT.md`에 package target으로 언급되지만 현재 구현은 Linux/macOS 중심이다. Windows 지원 여부를 제품 정책으로 결정해야 한다.

### 2.2 Controller/Agent 연결 구조

현재 구현:

- agent가 controller로 outbound 접속한다.
- controller가 agent로 직접 inbound 접속하지 않는다.
- persistent WebSocket session이 있다.
- agent reconnect가 가능하다.
- controller는 active session registry를 통해 연결된 agent에 즉시 task를 push한다.
- HTTP도 허용하되 test-only 경고를 출력한다.
- HTTPS/TLS 실행 경로가 있다.

PROJECT.md 목표:

- WebSocket over TLS 기반 outbound agent 연결
- real-time automation
- controller public key pinning
- signed task payload
- timeout/cancel
- production에서는 TLS 기본 통신
- network edge/server/NAT 환경에서 agent outbound 중심 운영

남은 개발:

- [ ] persistent session protocol versioning 정리
- [ ] task assignment ack/start/reject/result state를 명확한 protocol message로 분리
- [ ] cancel message와 agent process termination boundary 강화
- [ ] reconnect 이후 미완료 job 재동기화 정책
- [ ] duplicate session 정책 문서화와 테스트 보강
- [ ] controller restart 시 agent 재접속과 pending job 복구 검증
- [ ] WebSocket session에 capability negotiation 추가
- [ ] TLS certificate rotation 또는 fingerprint rotation 전략
- [ ] HTTP 사용 시 UI에서도 명확한 경고 표면 제공

우선순위 판단:

- phase003에서 즉시 push 기반은 구현되었다.
- 이제 중요한 것은 "task가 정확히 어떤 상태를 거쳐 실행되고, 끊겼을 때 어떻게 복구되는가"다.

### 2.3 Security, Approval, Audit

현재 구현:

- enrollment token을 통해 agent를 등록한다.
- admin token 기반 Web Admin 접근이 있다.
- task signature 검증 경계가 있다.
- controller fingerprint pinning이 있다.
- high-risk command confirmation 일부가 있다.
- revoke agent/key 흐름이 있다.
- HTTP 사용 경고와 일부 audit가 있다.
- secret redaction이 일부 경로에 있다.

PROJECT.md 목표:

- enrollment token은 1회성 또는 짧은 TTL 기반
- token 저장 시 hash 또는 안전한 secret storage
- agent identity key pair
- controller identity key pair
- controller-signed task envelope
- unsigned/invalid/expired/replayed/target mismatch task 거부
- approval required job flow
- append-only audit log
- secret redaction
- least privilege mode
- role-based admin UX
- OIDC/LDAP/SAML은 later지만 제품 확장 경로 필요

남은 개발:

- [ ] admin token만 쓰는 임시 모델에서 admin user/session/RBAC 모델로 확장
- [ ] CLI login/profile과 admin auth model 정리
- [ ] high-risk command confirmation을 approval workflow로 격상
- [ ] approval request, approval granted, approval rejected, approval expired 상태 추가
- [ ] approver identity와 audit event 연결
- [ ] audit event schema 정리
- [ ] audit append-only 보장 수준 정의
- [ ] audit export API
- [ ] secret redaction contract test 보강
- [ ] agent key rotation
- [ ] controller key rotation
- [ ] enrollment token revoke/expire UX 개선
- [ ] replay attack 방지 테스트 강화
- [ ] agent capability 기반 실행 제한
- [ ] least privilege execution mode 구체화

우선순위 판단:

- 원격 명령 실행 플랫폼에서 권한과 승인은 제품 안전성의 핵심이다.
- 현재 `confirmed_high_risk` 수준은 최소 방어선이고, 제품화에는 approval queue가 필요하다.

### 2.4 Job Lifecycle과 Multi-Agent Fanout

현재 구현:

- 단일 agent 대상 command 실행이 가능하다.
- 연결된 agent에는 immediate dispatch가 가능하다.
- job output을 Web Admin에서 확인할 수 있다.
- 일부 상태는 queued/running/success/failed/canceled/expired 중심이다.

PROJECT.md 목표:

- multi-agent automation
- inventory selector 기반 대상 선정
- job states: draft, pending_approval, queued, running, partial_success, success, failed, canceled
- concurrency
- max failures
- rollback/remediation 확장 가능성
- task output chunk와 result를 명확히 분리

남은 개발:

- [ ] job state machine을 domain layer에 명확히 고정
- [ ] assignment state machine 추가
- [ ] dispatched/accepted/started/rejected/completed 구분
- [ ] output chunk와 final result 분리
- [ ] agent disconnect 시 assignment 상태 처리
- [ ] multi-agent fanout API
- [ ] selector 결과 snapshot 저장
- [ ] fanout concurrency 제한
- [ ] maxFailures 정책
- [ ] partial_success 상태
- [ ] per-target result summary
- [ ] cancel propagation
- [ ] timeout policy
- [ ] retry policy
- [ ] Web Admin job detail에서 target별 결과 표시

우선순위 판단:

- 현재 real-time single-agent execution은 가능하다.
- Ansible류 제품으로 가려면 "여러 agent를 대상으로 안전하게 나눠 실행하고 요약하는 모델"이 필요하다.

### 2.5 Runbook DSL과 Execution Primitive

현재 구현:

- 제한된 runbook validation과 실행 경로가 있다.
- command 실행이 가능하다.
- package/service/file.copy 계열 primitive 일부가 있다.
- drift check job과 runbook path가 있다.

PROJECT.md 목표:

- YAML 기반 runbook DSL
- small primitive 중심
- idempotent execution
- package, service, file.copy, file.template, user, group, cron, command, shell, reboot, port.check, process.check, facts.collect, logs.tail, metrics.snapshot
- changed/skipped/failed result
- diff/check mode
- approval hook
- strategy/concurrency/maxFailures

남은 개발:

- [x] runbook schema versioning
- [x] DSL parser contract test
- [x] `matchLabels` object selector
- [x] `strategy.concurrency`
- [x] `strategy.maxFailures`
- [x] check mode/dry-run
- [x] primitive common result schema
- [x] idempotent changed/skipped model
- [ ] diff output model
- [ ] `file.template`
- [ ] `user`
- [ ] `group`
- [ ] `cron`
- [ ] `port.check`
- [ ] `process.check`
- [ ] `reboot` with explicit approval
- [ ] `shell` primitive를 command보다 높은 위험 등급으로 분리
- [ ] primitive별 timeout/output limit
- [ ] primitive별 least privilege requirement

우선순위 판단:

- primitive를 무작정 많이 늘리는 것보다 result model과 idempotency를 먼저 고정해야 한다.
- `shell`, `reboot`, `user`, `group`은 보안/승인 경계가 먼저 갖춰진 뒤 확장해야 한다.

### 2.6 Facts, Metrics, Logs, Drift

현재 구현:

- facts는 static inventory 성격으로 정리되어 memory/disk usage telemetry를 담지 않는다.
- metrics는 cpu/memory/disk usage 등 시계열 운영 값으로 분리되어 있다.
- facts/metrics/logs/drift paging API가 있다.
- agent system time과 stored at을 표시한다.
- Web Admin metrics chart가 있다.
- log upload interval이 독립적으로 설정 가능해졌다.

PROJECT.md 목표:

- facts.collect
- logs.tail
- metrics.snapshot
- drift detection
- audit와 연계
- operator가 언제 수집된 데이터인지 명확히 확인
- 오래된 데이터 retention

남은 개발:

- [x] facts schema를 static inventory 중심으로 더 안정화
- [x] disk inventory: physical disk, partition, mount, filesystem, total size, mount options
- [x] memory inventory: total, module count 가능 시 표시
- [ ] network inventory: interface address, mac, speed 가능 시 표시
- [x] metrics schema를 usage telemetry 중심으로 안정화
- [ ] metrics chart interval/range selector
- [ ] stale data 표시
- [ ] log source model 정의
- [ ] journald/systemd log adapter
- [ ] file tail adapter
- [ ] remote logs.tail task
- [x] facts/metrics/agent log explicit retention cleanup
- [ ] facts/metrics/log retention worker
- [ ] Prometheus/OpenTelemetry export 검토
- [ ] drift history diff UI 개선

우선순위 판단:

- facts와 metrics의 의미가 사용자 질문에서 이미 혼란을 만들었다.
- schema와 UI 라벨을 먼저 안정화해야 이후 API 호환성을 지킬 수 있다.

### 2.7 Inventory, Targeting, Capability

현재 구현:

- agent name/id/labels 기반 표시와 기본 selector가 있다.
- agent online/offline/revoked 상태 표시가 있다.

PROJECT.md 목표:

- agent fields: id, name, labels, version, capabilities, last_seen, status, policy_id
- selector: `agent:web-01`, `label:role=web`, `group:web`, query selector
- group과 saved selector
- capability 기반 안전 실행

남은 개발:

- [ ] agent version 수집 및 표시
- [ ] binary version과 protocol version 분리
- [ ] agent capability declaration
- [ ] OS/arch/runtime capability 표시
- [ ] primitive별 required capability
- [ ] policy_id field
- [ ] group object
- [ ] saved selector object
- [ ] selector parser를 domain layer로 이동/강화
- [ ] query selector grammar 설계
- [ ] Web Admin target preview
- [ ] selector result audit snapshot

우선순위 판단:

- multi-agent 실행 전 target preview와 selector snapshot이 필요하다.
- capability model은 least privilege와 primitive 확장 전에 필요하다.

### 2.8 Storage, Migration, Backup

현재 구현:

- SQLite 기반 controller store가 있다.
- data dir 기준 DB 생성/초기화가 있다.
- 일부 migration 또는 schema init 흐름이 있다.

PROJECT.md 목표:

- SQLite default
- Postgres optional
- local file artifact store
- SQLx/SeaORM migration
- backup command
- log retention
- production controller deployment 가능

남은 개발:

- [ ] repository contract 정리
- [ ] SQLite migration versioning 강화
- [ ] Postgres store 구현 여부 결정
- [ ] Postgres migration
- [ ] store contract test suite
- [ ] artifact storage abstraction
- [ ] job output artifact storage
- [x] backup command
- [x] restore command
- [ ] database integrity check
- [ ] retention policy
- [ ] retention worker
- [ ] storage metrics

우선순위 판단:

- 제품화 beta 전에는 backup/restore가 더 시급하다.
- Postgres는 팀/운영 규모가 커질 때 필요하지만, repository contract와 migration discipline은 지금 잡아야 한다.

### 2.9 Web Admin UI

현재 구현:

- controller에 static asset으로 내장/서빙된다.
- 별도 Node.js web server가 필요하지 않다.
- admin token 입력 후 agent/job/facts/metrics/drift/enrollment 일부를 관리한다.
- metrics chart가 있다.
- run command UI가 있다.

PROJECT.md 목표:

- lightweight Web Admin
- agent list
- job live output
- drift diff
- audit list
- approval 처리
- policy/runbook 최소 관리
- UI는 domain rule을 복제하지 않음

남은 개발:

- [ ] UI API client 에러 상태 정리
- [ ] 401/403/404/409 에러별 operator message
- [ ] HTTP transport 경고 banner
- [ ] selected agent stale/revoked/offline 상태 표시
- [ ] job output live UX 개선
- [ ] approval queue
- [ ] runbook catalog
- [ ] runbook dry-run result 표시
- [ ] policy assignment view
- [ ] target preview view
- [ ] audit timeline filter
- [ ] facts/metrics chart range selector
- [ ] empty state와 loading state 정리
- [ ] favicon/static asset completeness

우선순위 판단:

- UI는 기능을 무겁게 늘리기보다 operator가 혼동하지 않게 상태와 위험을 명확히 보여주는 것이 중요하다.
- approval queue와 target preview는 제품 안전성 측면에서 우선이다.

### 2.10 API, Swagger, SDK, Docs

현재 구현:

- OpenAPI/Swagger 문서가 있다.
- docs/api.md가 있다.
- README.md와 README.ko.md가 있다.
- release notes와 service install docs가 있다.

확인된 문서 갭:

- 일부 문서가 아직 MVP 기준 표현을 유지한다.
- `--dev-insecure-loopback` 제거 이후에도 오래된 예시가 남아 있을 수 있다.
- HTTP 원격 사용 정책이 바뀌었으므로 `PROJECT.md`와 일부 docs를 현재 구현 기준으로 동기화해야 한다.
- `docs/api.md`에 이미 구현된 drift/runbook 관련 내용을 미래형으로 설명하는 부분이 있다.
- `docs/logs.md`는 phase003 이후 독립 interval 모델을 반영해야 한다.
- `docs/release-notes-mvp.md`는 TLS/real-time dispatch/검증 스크립트 현황을 갱신해야 한다.
- `docs/service-install.md` 예시는 `agent enroll`보다 현재 권장 UX인 `agent init` 중심으로 정리하는 편이 낫다.
- `npm/fleet/README.md`는 package target을 "planned"가 아니라 현재 지원/미지원으로 구분해야 한다.

남은 개발:

- [ ] README.md 최신 실행 흐름 정리
- [ ] README.ko.md와 README.md 동기화
- [ ] HTTP 사용 경고 문구 일관화
- [ ] HTTPS 준비 절차 별도 섹션 유지
- [ ] controller/agent 초기화/삭제/재등록 flow 정리
- [ ] API docs를 현재 구현과 맞춤
- [ ] OpenAPI endpoint coverage 확인
- [ ] Swagger UI 접근 문서 정리
- [ ] API pagination contract 명확화
- [ ] facts/metrics/drift time fields 문서화
- [ ] release notes 갱신
- [ ] npm README 갱신
- [ ] troubleshooting 갱신
- [ ] docs stale scan을 release gate에 포함

우선순위 판단:

- 현재 사용자 질문 대부분이 용어, 실행 절차, HTTP/HTTPS, agent 등록/상태, facts/metrics 의미에서 발생했다.
- 제품화 전에는 문서가 구현보다 뒤처지면 사용자 신뢰가 크게 떨어진다.

## 3. 다음 개발의 제품 목표

다음 단계의 제품 목표는 다음과 같다.

```text
MVP를 넘어, 하나의 controller에 여러 agent를 안정적으로 붙이고,
운영자가 Web Admin과 CLI/API를 통해 안전하게 대상 선정, 승인, 실행,
상태 확인, 감사 추적, 복구 판단을 할 수 있는 beta 제품 기반을 만든다.
```

이를 위해 다음 원칙을 유지한다.

- 단일 바이너리 `sponzey` 유지
- agent outbound connection 유지
- controller가 agent로 inbound 연결하지 않음
- HTTP는 기술적으로 허용하되 test-only 경고 유지
- production 설명은 HTTPS 중심
- runtime env patch 금지
- external settings 최소화
- signed task envelope 유지
- audit 누락 없이 위험 기능 추가
- UI는 얇은 운영 표면으로 유지
- 핵심 로직은 domain/application test 우선

### 3.1 계획 재검토 후 보정 결론

이 계획을 다시 읽어보면 큰 방향은 맞지만, 단순히 기능 목록을 늘리는 방식으로 진행하면 위험하다. Sponzey Fleet는 원격 실행 제품이므로 다음 순서가 반드시 지켜져야 한다.

1. 문서와 현재 구현의 기준선을 맞춘다.
2. job/assignment 상태 모델을 먼저 고정한다.
3. selector와 target snapshot을 고정한다.
4. 승인/audit/capability 없이 위험 primitive를 늘리지 않는다.
5. runbook DSL은 result schema와 idempotency를 먼저 고정한 뒤 primitive를 확장한다.
6. storage/retention/backup은 production beta 전 필수로 묶는다.
7. packaging/upgrade는 기능 개발 마지막이 아니라 production beta gate의 일부로 본다.

보정해야 하는 점:

- Phase 004는 단순 문서 정리가 아니라 "현재 구현 기준선 확정"이어야 한다.
- Phase 005와 Phase 006은 분리하되, 같은 제품 기능인 "안전한 실행 모델"로 묶어 검증해야 한다.
- Phase 007 approval은 UI 기능이 아니라 domain/application 보안 기능으로 먼저 구현해야 한다.
- Phase 008 primitive 확장은 approval/capability가 없는 상태에서는 safe primitive에 한정해야 한다.
- Phase 011 storage readiness는 너무 늦게 밀리면 안 된다. job/assignment migration이 들어가기 전 repository/migration 규칙을 함께 점검해야 한다.
- Phase 014 Web Admin UX는 마지막 polish가 아니라 각 domain 기능을 얇게 노출하는 검증 표면으로 계속 동반되어야 한다.

### 3.2 제품화 불변 조건

다음 조건은 이후 모든 phase에서 깨지면 안 된다.

- Controller는 Agent로 직접 inbound 접속하지 않는다.
- Agent는 Controller에 outbound persistent session을 유지한다.
- heartbeat는 task dispatch 수단이 아니라 liveness signal이다.
- task dispatch는 active session을 사용하되, DB에 job/assignment를 먼저 기록한 뒤 수행한다.
- active session 존재는 approval, signature, expiry, nonce replay, target 검증을 우회하는 이유가 될 수 없다.
- job은 사용자/운영자 관점의 실행 단위이고, assignment는 특정 agent에 대한 실행 단위다.
- output chunk와 final result는 protocol과 storage에서 구분한다.
- facts는 거의 변하지 않는 inventory이고, metrics는 시간에 따라 변하는 usage telemetry다.
- logs는 application log가 아니라 agent가 올리는 operational event/log stream이며, 원문 command output과 섞지 않는다.
- HTTP는 허용하지만 test-only warning과 audit를 유지한다.
- production 설명은 HTTPS 중심으로 유지한다.
- UI는 상태와 위험을 보여줄 뿐 권한 판단을 하지 않는다.
- runtime env patch나 숨은 config file 증가는 허용하지 않는다.

### 3.3 Phase Gate

각 phase는 "구현 완료"가 아니라 "제품 기준으로 다음 phase에 넘어가도 되는가"를 기준으로 닫는다.

Phase 004 gate:

- [ ] README/README.ko/docs/PROJECT의 실행 흐름과 정책이 충돌하지 않는다.
- [ ] stale CLI option scan이 통과한다.
- [ ] 현재 API와 OpenAPI/docs의 큰 불일치가 목록화되어 있다.
- [ ] release gate에서 반드시 돌릴 cargo/npm/smoke 명령이 정리되어 있다.

Phase 005 gate:

- [ ] job state와 assignment state가 domain test로 고정된다.
- [ ] agent ack/start/reject/result가 protocol에서 구분된다.
- [ ] reconnect/cancel/timeout 동작이 테스트된다.
- [ ] 기존 single-agent run UX가 깨지지 않는다.

Phase 006 gate:

- [ ] selector preview와 target snapshot이 있다.
- [ ] multi-agent fanout이 concurrency/maxFailures를 지킨다.
- [ ] target별 결과와 전체 job 결과가 일관되게 계산된다.
- [ ] UI/API에서 partial_success를 설명할 수 있다.

Phase 007 gate:

- [ ] 위험 작업은 approval 없이 실행되지 않는다.
- [ ] approver identity와 audit event가 남는다.
- [ ] UI가 approval 상태를 자체 판단하지 않는다.
- [ ] 기존 `confirmed_high_risk` 경로는 새 approval 모델로 흡수되거나 호환 정책이 명확하다.

Phase 008 gate:

- [ ] runbook schema와 primitive result schema가 fixture test로 고정된다.
- [ ] safe primitive는 idempotent changed/skipped/failed를 제공한다.
- [ ] dangerous primitive는 approval/capability 없이 열리지 않는다.
- [ ] dry-run/check mode가 side effect를 만들지 않는다.

Phase 011 gate:

- [ ] migration test가 empty DB와 previous schema fixture 모두에서 통과한다.
- [x] backup/restore roundtrip이 통과한다.
- [ ] retention worker가 audit를 삭제하지 않는다.
- [ ] data dir 초기화/삭제/복구 문서가 명확하다.

Phase 012 gate:

- [ ] npm install 외 공식 artifact 설치 경로가 최소 하나 이상 검증된다.
- [ ] service install/uninstall/status가 smoke test를 가진다.
- [ ] checksum/signature 또는 그에 준하는 release integrity 검증 경로가 있다.
- [ ] upgrade 실패 시 rollback 또는 recovery 문서가 있다.

### 3.4 다음 task 작성 원칙

이 계획을 task 파일로 쪼갤 때는 다음을 강제한다.

- task 하나는 기능 2~3개만 묶는다.
- 각 task에는 "구현", "테스트", "문서", "검증 명령", "완료 기준"을 모두 둔다.
- security, token, task execution, approval, migration 관련 task는 테스트 없는 완료 처리를 금지한다.
- UI task도 API error handling과 최소 smoke test를 포함한다.
- task가 기존 동작을 바꾸면 regression test를 먼저 적는다.
- release 또는 packaging task는 실제 install smoke 또는 artifact smoke를 포함한다.

## 4. Phase 004: 문서와 제품 기준선 정리

목표:

현재 구현과 문서가 어긋난 부분을 먼저 정리한다. 제품화 단계에서는 문서가 설치/운영의 일부이므로, 오래된 CLI 옵션이나 정책 설명을 남기지 않는다. 또한 이후 phase에서 기준으로 삼을 현재 기능 matrix와 release gate를 확정한다.

기능 묶음:

1. README와 운영 시작 흐름 정리
2. docs/api, docs/logs, release notes 최신화
3. 구현 기준선과 release gate 정리

상세 작업:

- [ ] README.md에서 controller/agent 용어를 다시 명확히 설명
- [ ] README.md에서 "controller는 중앙 서버, agent는 대상 서버" 구조를 첫 부분에 명시
- [ ] README.md에서 HTTP는 사용 가능하지만 test-only라는 경고를 일관되게 유지
- [ ] README.md에서 HTTPS 준비는 별도 섹션으로 분리
- [ ] README.md에서 `--dev-insecure-loopback` 예시 제거 여부 확인
- [ ] README.ko.md를 README.md와 내용 동기화
- [ ] `docs/api.md`에서 이미 구현된 endpoint와 미구현 endpoint를 구분
- [x] `docs/api.md`에서 facts/metrics/logs/drift paging contract 명시
- [x] `docs/api.md`에서 `agent_system_time_ms`, `stored_at` 의미 설명
- [x] `docs/logs.md`에서 log interval이 heartbeat와 독립된 현재 구조 반영
- [ ] `docs/service-install.md`에서 `agent init` 중심 예시로 정리
- [ ] `docs/release-notes-mvp.md`에서 v0.0.14 기준 기능 업데이트
- [ ] `npm/fleet/README.md`에서 지원 platform과 미지원 platform 명시
- [ ] `PROJECT.md`에서 HTTP 원격 사용 정책이 현재 구현과 충돌하는지 확인
- [ ] `PROJECT.md`에서 `--dev-insecure-loopback` 잔여 표현 제거 또는 과거 설명으로 이동
- [ ] `PROJECT.md`에서 phase003 persistent connection 반영
- [ ] 현재 구현 feature matrix 작성
- [ ] 구현됨/부분 구현/미구현/정책 결정 필요 상태를 구분
- [ ] controller/agent/web-admin/npm/docs별 release gate 명령 정리
- [ ] stale docs scan keyword 목록 정리
- [ ] API docs와 OpenAPI endpoint coverage gap 목록화

검증:

- [ ] `rg -n "dev-insecure-loopback|insecure remote HTTP|planned release package"` 결과 확인
- [ ] README.md와 README.ko.md 주요 명령이 동일한 의미인지 확인
- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `npm test --workspace @sponzey/fleet`
- [ ] 현재 존재하는 smoke script 목록 확인
- [ ] `git diff --check`

완료 기준:

- 사용자가 문서만 보고 controller start, enrollment token 생성, agent init/start, Web Admin 접속, agent 삭제/재등록 흐름을 이해할 수 있다.
- HTTP/HTTPS 설명이 중복 나열이 아니라 "기본 흐름 + HTTPS 준비" 구조로 정리되어 있다.
- 현재 없는 옵션이나 미래형 설명이 주요 getting started 경로에 남아 있지 않다.
- 다음 phase에서 기준으로 삼을 "현재 가능한 것과 아직 안 되는 것"이 문서에 드러난다.
- release 전에 반드시 실행할 검증 명령이 명확하다.

## 5. Phase 005: Job Lifecycle과 Assignment State Machine

목표:

현재 immediate dispatch를 제품 수준의 job/assignment lifecycle로 확장한다. "명령을 보냈다"가 아니라 "어떤 agent가 수락했고, 시작했고, 실패했고, 결과를 보냈는지"를 정확히 추적한다.

기능 묶음:

1. domain job state machine 정리
2. assignment state machine 추가
3. reconnect/retry/cancel 정책 정리

상세 작업:

- [ ] `JobStatus` domain enum 재검토
- [ ] `draft`, `pending_approval`, `queued`, `running`, `partial_success`, `success`, `failed`, `canceled`, `expired` 지원 범위 결정
- [ ] 현재 구현 상태와 migration 호환성 확인
- [ ] `AssignmentStatus` domain enum 추가
- [ ] `queued`
- [ ] `dispatched`
- [ ] `accepted`
- [ ] `started`
- [ ] `output_received`
- [ ] `succeeded`
- [ ] `failed`
- [ ] `rejected`
- [ ] `canceled`
- [ ] `expired`
- [ ] agent가 task를 받으면 ack message 전송
- [ ] agent가 실행 시작 시 started message 전송
- [ ] agent가 policy/capability 문제로 거부 시 rejected message 전송
- [ ] output chunk와 final result protocol 분리
- [ ] controller가 assignment transition을 domain rule로 검증
- [ ] invalid transition은 audit/security event로 남김
- [ ] agent reconnect 시 in-flight assignment 처리 정책 정의
- [ ] controller restart 후 queued/running assignment 복구 정책 정의
- [ ] cancel request protocol message 구현
- [ ] agent process runner cancel/kill boundary 구현
- [ ] timeout과 cancel의 결과 차이를 UI/API에 표시

테스트:

- [ ] domain state transition unit test
- [ ] invalid transition rejection test
- [ ] disconnected agent queued assignment test
- [ ] connected agent immediate dispatch ack test
- [ ] reconnect after dispatch test
- [ ] cancel before start test
- [ ] cancel while running test
- [ ] timeout result test
- [ ] Web Admin selected job status rendering smoke

완료 기준:

- job과 assignment 상태가 혼동되지 않는다.
- agent가 task를 받았는지, 실행했는지, 거부했는지, 실패했는지가 API와 UI에서 구분된다.
- disconnected/reconnected 상황에서도 controller가 무리하게 성공 처리하지 않는다.

## 6. Phase 006: Multi-Agent Fanout과 Targeting

목표:

Ansible류 제품으로 가기 위해 여러 agent를 대상으로 한 번의 job을 안전하게 실행한다.

기능 묶음:

1. selector result snapshot
2. fanout concurrency/maxFailures
3. target별 result summary

상세 작업:

- [ ] selector parser를 domain/application 경계로 정리
- [ ] `agent:<name-or-id>` selector 정리
- [ ] `label:key=value` selector 정리
- [ ] `matchLabels` object selector 추가
- [ ] selector preview API
- [ ] selector result snapshot 저장
- [ ] job 생성 시 target snapshot 고정
- [ ] 실행 중 labels 변경이 기존 job target에 영향을 주지 않도록 보장
- [ ] fanout dispatcher 구현
- [ ] concurrency limit
- [ ] maxFailures limit
- [ ] unreachable/offline target handling
- [ ] partial_success 계산
- [ ] target별 assignment summary API
- [ ] Web Admin target preview
- [ ] Web Admin job target result table
- [ ] CLI에서 selector preview 또는 dry-run 제공

테스트:

- [ ] selector matching unit test
- [ ] selector snapshot persistence test
- [ ] labels changed after job creation test
- [ ] fanout concurrency test
- [ ] maxFailures stop test
- [ ] partial_success aggregation test
- [ ] offline target summary test
- [ ] Web Admin target preview smoke

완료 기준:

- 운영자가 실행 전에 대상 목록을 확인할 수 있다.
- 실행 중 대상이 흔들려도 job target이 바뀌지 않는다.
- 일부 agent 실패가 전체 job 상태에 어떻게 반영되는지 명확하다.

## 7. Phase 007: Approval Workflow와 Admin Authorization

목표:

`confirmed_high_risk` 수준의 임시 보호를 제품형 approval workflow로 격상한다.

기능 묶음:

1. approval queue
2. admin identity/session/RBAC 기초
3. audit hardening

상세 작업:

- [ ] `ApprovalRequest` domain entity 추가
- [ ] approval required classification 정리
- [ ] high-risk command
- [ ] shell primitive
- [ ] reboot primitive
- [ ] user/group primitive
- [ ] broad target selector
- [ ] root-required task
- [ ] approval request 생성 use case
- [ ] approval approve/reject use case
- [ ] approval expiry
- [ ] approval comment/reason
- [ ] approver identity 저장
- [x] admin token 모델과 admin identity 모델의 관계 정리
- [x] 초기 bootstrap admin token은 유지하되 product admin으로 확장 가능하게 설계
- [x] CLI login/profile UX 재검토
- [x] RBAC role 초안
- [x] owner/admin/operator/viewer
- [x] permission matrix 초안
- [ ] Web Admin approval queue
- [ ] approval detail modal/page
- [ ] 승인 후 job dispatch 연결
- [ ] reject/cancel audit
- [ ] audit event type 정리
- [ ] audit export API 초안

테스트:

- [ ] approval required classification unit test
- [ ] approval create/approve/reject application test
- [ ] expired approval cannot dispatch test
- [ ] rejected approval cannot dispatch test
- [x] permission denied API test
- [ ] audit event creation test
- [ ] Web Admin approval queue smoke

완료 기준:

- 위험 작업은 단순 checkbox가 아니라 별도 approval lifecycle을 가진다.
- 누가 승인했는지 audit로 남는다.
- UI는 권한을 결정하지 않고 controller 응답을 표현한다.

## 8. Phase 008: Runbook DSL과 Idempotent Primitive 확장

목표:

Sponzey Fleet의 자동화 언어를 단순 command runner가 아니라 idempotent runbook으로 확장한다.

기능 묶음:

1. runbook schema/result model 안정화
2. safe primitive 우선 확장
3. dangerous primitive는 approval과 capability 이후 확장

상세 작업:

- [x] runbook schema version 필수화
- [x] parser error를 사용자 친화적으로 정리
- [x] DSL fixture test 추가
- [x] `name`, `description`, `selector`, `strategy`, `steps` 구조 안정화
- [x] `strategy.concurrency`
- [x] `strategy.maxFailures`
- [x] `checkMode`
- [x] `dryRun`
- [x] primitive result common schema
- [x] changed/skipped/failed
- [x] diff
- [x] message
- [x] started_at/completed_at
- [x] duration_ms
- [x] command primitive result 정리
- [x] package primitive idempotency 강화
- [x] service primitive idempotency 강화
- [x] file.copy checksum/diff 강화
- [ ] file.template 설계
- [x] port.check primitive
- [x] process.check primitive
- [x] facts.collect primitive
- [x] metrics.snapshot primitive
- [ ] logs.tail primitive
- [ ] user/group/cron/reboot/shell은 approval/capability 이후 구현

테스트:

- [x] valid runbook fixture test
- [x] invalid runbook fixture test
- [x] backward compatibility fixture test
- [x] primitive result schema test
- [x] package idempotency test
- [x] service idempotency test
- [x] file.copy checksum test
- [x] check mode no-change test
- [x] dry-run no-side-effect test

완료 기준:

- runbook 실행 결과가 사람과 API client 모두 이해 가능한 구조다.
- idempotent primitive는 changed 여부를 신뢰할 수 있다.
- 위험 primitive는 승인/권한 모델 없이 열리지 않는다.

## 9. Phase 009: Policy, Drift, Remediation 제품 루프

목표:

drift detection을 단발성 확인이 아니라 policy 기반 운영 루프로 만든다.

기능 묶음:

1. policy object와 assignment
2. scheduled drift check
3. manual remediation flow

상세 작업:

- [x] `Policy` domain entity 추가
- [x] policy versioning
- [x] policy source 저장
- [x] policy validation
- [x] policy assignment to agent
- [ ] policy assignment to group/selector
- [x] policy_id를 agent inventory/API에 반영
- [ ] scheduled drift check worker
- [x] check interval policy
- [x] missed schedule handling
- [x] drift report history
- [x] latest drift와 history API 구분
- [x] drift severity model
- [x] drift acknowledged state
- [x] remediation runbook 연결
- [x] remediation approval request
- [x] remediation result와 drift resolution 연결
- [ ] Web Admin policy list
- [ ] Web Admin policy assignment
- [ ] Web Admin drift history
- [ ] Web Admin remediation action

테스트:

- [x] policy validation unit test
- [x] policy assignment application test
- [x] scheduled drift scheduler boundary test with fake clock
- [ ] scheduled drift worker test with fake clock
- [x] drift history paging test
- [x] remediation approval required test
- [x] remediation result updates drift state test
- [ ] Web Admin drift/policy smoke

완료 기준:

- operator가 "이 agent는 어떤 정책을 따라야 하는지" 확인할 수 있다.
- drift는 latest snapshot만이 아니라 history로 추적된다.
- remediation은 자동 실행이 아니라 승인 가능한 작업으로 연결된다.

## 10. Phase 010: Agent Capability와 Least Privilege

목표:

agent가 무엇을 할 수 있고 무엇을 하면 안 되는지 controller와 agent 모두가 명확히 알게 한다.

기능 묶음:

1. capability declaration
2. primitive required capability
3. least privilege execution mode

상세 작업:

- [ ] agent startup 시 capability 수집
- [ ] OS/arch/runtime capability
- [ ] package manager capability
- [ ] service manager capability
- [ ] filesystem write capability
- [ ] process execution capability
- [ ] privilege level
- [ ] sudo/root 여부
- [ ] capability protocol message
- [ ] capability persistence
- [ ] capability UI 표시
- [ ] primitive별 required capability 정의
- [ ] capability mismatch 시 assignment rejected
- [ ] least privilege mode config
- [ ] root-required primitive 분류
- [ ] sudo/su 정책 재검토
- [ ] child process env/working dir 제한
- [ ] output size limit 일관화
- [ ] process timeout 일관화

테스트:

- [ ] capability collection unit/application test
- [ ] required capability matching test
- [ ] capability mismatch rejected test
- [ ] non-root agent cannot run root-required primitive test
- [ ] output limit test
- [ ] timeout boundary test

완료 기준:

- controller는 agent가 수행할 수 없는 task를 무리하게 보내지 않거나, agent가 명확히 거부한다.
- root 권한이 필요한 작업은 capability/approval/audit 경계를 모두 지난다.

## 11. Phase 011: Storage Production Readiness

목표:

SQLite 개발/소규모 운영 경로를 유지하면서 production controller 운영에 필요한 backup, retention, migration discipline을 갖춘다.

기능 묶음:

1. migration/repository contract
2. backup/restore
3. retention worker

상세 작업:

- [ ] SQLite schema version table 정리
- [ ] migration 파일 또는 코드 migration 체계 고정
- [ ] repository trait contract 정리
- [ ] store contract test suite 작성
- [ ] DB initialization과 migration을 명확히 분리
- [x] `sponzey controller backup`
- [x] `sponzey controller restore`
- [x] backup metadata
- [x] backup integrity check
- [x] restore dry-run
- [ ] retention policy object
- [x] job output retention
- [x] metrics retention
- [x] logs retention
- [x] audit retention 정책은 별도 취급
- [ ] retention worker
- [x] manual retention command
- [ ] Postgres 지원 여부 결정 문서
- [ ] Postgres repository spike 또는 task 분리

테스트:

- [ ] migration from empty DB test
- [ ] migration from previous schema fixture test
- [x] backup/restore roundtrip test
- [x] restore dry-run test
- [x] retention fake clock test
- [x] audit not accidentally deleted test

완료 기준:

- 운영자가 controller data dir을 잃기 전에 backup할 수 있다.
- 오래된 metrics/logs/job output이 무한정 쌓이지 않는다.
- schema 변경이 ad hoc이 아니라 검증 가능한 migration으로 들어간다.

## 12. Phase 012: Packaging, Service, Upgrade

목표:

npm install 외에도 운영 서버에 맞는 설치와 서비스 등록, 업그레이드 경로를 제공한다.

기능 묶음:

1. service install hardening
2. standalone/package artifact
3. upgrade channel

상세 작업:

- [x] Linux systemd unit template 정리
- [x] controller service install command
- [x] agent service install command
- [x] service uninstall command
- [x] service status command
- [x] service log command
- [x] macOS launchd 지원 여부 결정
- [x] Windows Service 지원 여부 결정
- [x] standalone tar.gz artifact 공식화
- [x] SHA256SUMS 검증 문서
- [x] signature 생성/검증 경로
- [ ] `.deb` package
- [ ] `.rpm` package
- [ ] Docker image
- [ ] Homebrew formula
- [ ] one-line installer
- [ ] install dry-run
- [ ] install version pinning
- [x] `sponzey upgrade`
- [x] stable/beta channel
- [x] rollback strategy

결정 사항:

- `sponzey upgrade`는 현재 자동 self-upgrade가 아니라 `--dry-run` planning command로 제한한다.
- signature 생성/검증은 현재 구현하지 않고 후속 hardening task로 분리한다. 이번 단계에서는 `SHA256SUMS` 검증과 release provenance 확인을 공식 integrity 경계로 둔다.
- macOS launchd, Windows Service, `.deb`, `.rpm`, Docker, Homebrew, one-line installer는 이번 단계에서 미지원 또는 후속 packaging task로 명확히 분리한다.

테스트:

- [x] service install smoke
- [x] service uninstall smoke
- [x] npm global install smoke
- [x] standalone archive smoke
- [x] checksum verification test
- [ ] Docker startup smoke if Docker path is added
- [x] release readiness gate update

완료 기준:

- 사용자는 npm이 아니어도 공식 경로로 설치할 수 있다.
- agent/controller를 OS service로 등록/삭제할 수 있다.
- 업그레이드 전후 데이터와 서비스 상태를 예측할 수 있다.

## 13. Phase 013: Public API, OpenAPI, SDK

목표:

외부 시스템이 Sponzey Fleet를 안정적으로 제어할 수 있는 API 표면을 정리한다.

기능 묶음:

1. OpenAPI coverage
2. API compatibility policy
3. generated client/SDK 준비

상세 작업:

- [x] 모든 public API endpoint inventory 작성
- [x] admin-only API와 agent protocol API 구분
- [x] public/stable/internal endpoint 구분
- [x] OpenAPI schema generation 정리
- [x] Swagger UI 접근 경로 문서화
- [x] pagination 공통 모델
- [x] error response 공통 모델
- [x] auth error response 모델
- [x] job/assignment response 모델
- [x] facts/metrics/logs/drift page response 모델
- [x] OpenAPI example payload 추가
- [x] API compatibility policy
- [x] deprecation policy
- [x] TypeScript client 생성 검토
- [x] Rust client crate 검토
- [x] CLI가 public API client를 재사용하도록 정리

테스트:

- [x] OpenAPI schema snapshot test
- [x] endpoint coverage test
- [x] pagination contract test
- [x] error response contract test
- [x] generated client smoke if added

Task 016 결과:

- OpenAPI는 hand-maintained `docs/openapi.json`을 controller가 `/openapi.json`으로 서빙한다.
- REST route contract test가 public/admin/agent protocol surface와 bearer auth 요구 여부를 검증한다.
- `/api/agents/ws`와 `/admin/*`는 REST API가 아니므로 OpenAPI 범위에서 제외하고 별도 문서로 분리한다.
- TypeScript generated SDK와 Rust client crate는 이번 단계에서 만들지 않는다. Web Admin의 dependency-free `api-client.js`와 `web-admin/api.schema.json`이 최소 client contract smoke 역할을 하며, CLI 공용 client crate는 API 호출량이 늘어나는 후속 단계에서 분리한다.

완료 기준:

- Swagger를 보고 외부 개발자가 API 사용 흐름을 이해할 수 있다.
- breaking change가 발생하는 지점을 release 전에 감지할 수 있다.

## 14. Phase 014: Web Admin Product UX

목표:

Web Admin을 단순 개발 화면에서 운영자가 반복 사용 가능한 얇은 관리 화면으로 다듬는다.

기능 묶음:

1. agent/job 상태 가시성
2. approval/policy/runbook 화면
3. telemetry 화면 정리

상세 작업:

- [x] agent list status badge 정리
- [x] revoked/offline/online/stale 상태 명확화
- [x] agent detail refresh UX 정리
- [x] selected agent 변경 bug regression test
- [x] job live output UI 개선
- [x] no output/loading/completed 상태 구분
- [x] target별 assignment table
- [x] approval queue
- [x] runbook catalog
- [x] runbook upload/validate
- [x] runbook dry-run result
- [x] policy list
- [x] policy assignment
- [x] drift history
- [x] metrics range selector
- [x] facts inventory grouping
- [x] disk/mount inventory table
- [x] log viewer
- [x] HTTP warning banner
- [x] admin auth expired 상태 처리

테스트:

- [x] API client unit test
- [x] agent list rendering test
- [x] selected agent switching test
- [x] revoked agent display test
- [x] job output rendering test
- [x] metrics chart rendering smoke
- [x] approval queue rendering test
- [x] UI build test

Task 017 결과:

- Web Admin은 agent detail, stale/revoked 표시, HTTP warning banner, disk/mount inventory table, metrics range selector, drift history, agent logs, target assignment table을 표시한다.
- Approval queue는 pending approval 조회, approve/reject, expire due action을 제공한다.
- Runbook 화면은 YAML 입력으로 signed runbook job을 만들고 result/status를 표시한다. 별도 catalog 저장소나 server-side validate-only endpoint는 아직 없으므로 full catalog/validate 제품화는 후속 범위다.
- Policy 화면은 policy source 저장, list, selected agent assignment, drift schedule action을 제공한다.
- UI는 authorization이나 risk/domain rule을 자체 판단하지 않고 controller API 응답과 상태를 표시한다.

완료 기준:

- 운영자가 agent와 job 상태를 혼동하지 않는다.
- 위험 작업은 UI에서도 승인 흐름을 통해 보인다.
- UI는 domain rule을 복제하지 않고 API 결과를 정확히 표시한다.

## 15. Phase 015: Enterprise/Scale Later

목표:

초기 beta 이후 조직/대규모 운영에 필요한 확장 지점을 준비한다. 이 단계는 지금 당장 구현보다 architecture decision record와 spike 중심으로 접근한다.

기능 묶음:

1. HA controller model
2. enterprise auth integration
3. large fleet scalability

상세 작업:

- [ ] Postgres 기반 HA controller 가능성 분석
- [ ] session registry externalization 필요성 검토
- [ ] queue/broker 필요성 검토
- [ ] sticky session 요구사항 검토
- [ ] OIDC integration spike
- [ ] LDAP/SAML later decision
- [ ] organization/project model
- [ ] multi-tenant boundary
- [ ] large fleet heartbeat load test
- [ ] metrics/log ingestion backpressure
- [ ] agent update campaign model
- [ ] audit archive/export model

완료 기준:

- enterprise 기능을 지금 당장 구현하지 않아도, 현재 구조가 나중에 막히지 않는지 판단할 수 있다.

## 16. 가까운 우선순위 제안

다음 순서가 가장 현실적이다.

1. Phase 004 문서/제품 기준선 정리
2. Phase 011 중 migration/repository contract 최소 규칙 선적용
3. Phase 005 job/assignment lifecycle
4. Phase 006 multi-agent fanout/targeting
5. Phase 007 approval/RBAC/audit
6. Phase 008 runbook DSL/idempotent primitive
7. Phase 011 storage backup/retention 완성
8. Phase 012 packaging/upgrade

이 순서를 추천하는 이유:

- 문서가 현재 구현과 다르면 사용자 설치/운영 질문이 계속 발생한다.
- job/assignment schema 변경 전에 migration/repository 규칙을 먼저 잡아야 한다.
- multi-agent 자동화 전에 job/assignment 상태가 먼저 안정되어야 한다.
- 위험 작업을 늘리기 전에 approval/audit가 필요하다.
- runbook primitive 확장은 idempotent result model이 먼저 잡혀야 한다.
- production beta 전에는 backup/retention/upgrade가 필요하다.

주의:

- Phase 번호는 제품 영역을 나타내며, 실제 작업 순서는 의존성에 따라 일부 앞당길 수 있다.
- 특히 Phase 011의 migration/repository contract는 Phase 005의 선행 조건이다.
- 반대로 Phase 011의 backup/retention 전체 완성은 Phase 005 이후에도 진행할 수 있다.

## 17. 다음 task 파일 분리 기준

이 plan을 task로 나눌 때는 기존 규칙을 유지한다.

- task 하나는 기능 2~3개 단위로 묶는다.
- 각 task는 구현, 테스트, 검증, 문서 업데이트 체크박스를 포함한다.
- 한 task 안에서 tidy와 behavior 변경을 섞으면 섹션을 나눈다.
- security/token/task execution 관련 task는 테스트 없이 완료 처리하지 않는다.

추천 task 분리:

- `task001.md`: 문서 최신화, 구현 feature matrix, release gate 정리
- `task002.md`: migration/repository contract 최소 규칙과 schema 변경 준비
- `task003.md`: job/assignment state machine domain test
- `task004.md`: protocol ack/start/reject/result 분리
- `task005.md`: cancel/timeout/reconnect 복구
- `task006.md`: selector preview와 target snapshot
- `task007.md`: fanout concurrency/maxFailures/partial_success
- `task008.md`: approval request lifecycle
- `task009.md`: admin auth/RBAC 초안과 permission check
- `task010.md`: runbook schema/result model
- `task011.md`: safe primitive 확장
- `task012.md`: policy assignment와 scheduled drift
- `task013.md`: facts/metrics/log schema와 retention
- `task014.md`: backup/restore
- `task015.md`: packaging/service/upgrade
- `task016.md`: OpenAPI/SDK contract
- `task017.md`: Web Admin product UX

## 18. 명확한 Non-Goals

다음은 지금 단계에서 목표가 아니다.

- Ansible full compatibility
- Kubernetes-first orchestrator
- 무제한 remote shell platform
- UI 기반 복잡한 workflow designer
- 운영 중 process env patch
- controller가 agent로 inbound 접속하는 구조
- approval 없는 자동 remediation
- 권한/감사 없는 root-level primitive 확장
- domain rule을 Web Admin에 복제
- Windows production support를 준비 없이 암묵적으로 지원한다고 표기

## 19. 완료 정의

Phase 004+ 전체가 제품화 beta 기준으로 의미 있으려면 다음이 충족되어야 한다.

- [ ] 문서가 현재 구현과 충돌하지 않는다.
- [ ] controller/agent 설치와 초기화 흐름을 초보자도 따라할 수 있다.
- [ ] HTTP는 사용 가능하지만 test-only 경고가 항상 명확하다.
- [ ] HTTPS production 준비 절차가 별도로 명확하다.
- [ ] job과 assignment 상태가 분리되어 있다.
- [ ] multi-agent fanout이 target snapshot 기반으로 동작한다.
- [ ] 위험 작업은 approval workflow를 거친다.
- [ ] audit event가 누락되지 않는다.
- [ ] runbook primitive는 idempotent result를 제공한다.
- [ ] drift는 policy와 remediation으로 연결된다.
- [ ] facts와 metrics 의미가 API/UI/문서에서 일관된다.
- [x] controller data backup/restore가 가능하다.
- [ ] retention으로 telemetry/job output이 무한 증가하지 않는다.
- [ ] npm 외 설치/서비스/업그레이드 경로가 명확하다.
- [ ] OpenAPI가 실제 API와 맞다.
- [ ] Web Admin은 운영자가 상태와 위험을 혼동하지 않게 만든다.

## 20. 첫 실행 제안

바로 다음 작업은 Phase 004부터 시작한다.

Phase 004는 기능 추가가 아니라 제품 기준선 정리다. 하지만 지금은 매우 중요하다. 현재 구현은 MVP보다 앞서간 부분이 있고, 문서는 일부 과거 정책과 미래형 설명을 함께 담고 있다. 이 상태에서 새로운 기능을 계속 추가하면 사용자와 개발자가 서로 다른 제품을 보고 있다고 생각하게 된다.

따라서 다음 turn에서 권장하는 첫 task는 다음이다.

```text
.tasks/task001.md 작성 또는 진행:
문서 최신화, 구현 feature matrix, release gate 정리
```

이 task에서 README, README.ko, docs/api, docs/logs, service install docs, npm README, PROJECT.md의 충돌 지점을 정리한다. 동시에 현재 구현된 기능과 부분 구현된 기능, 아직 미구현인 기능을 feature matrix로 정리하고, release 전에 반드시 실행해야 하는 검증 명령과 smoke script를 고정한다.

그 다음 바로 기능 구현으로 넘어가지 않는다. `task002.md`에서 migration/repository contract 최소 규칙을 먼저 확인한 뒤, `task003.md`부터 job/assignment lifecycle 구현으로 들어간다.
