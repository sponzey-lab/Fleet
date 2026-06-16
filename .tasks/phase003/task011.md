# Task 011 - 문서, Swagger, Smoke

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

상시 Agent 연결 모델을 문서, Swagger/OpenAPI, smoke test로 닫는다.

사용자는 "Controller가 Agent로 접속한다"가 아니라 "Agent가 Controller에 outbound persistent WebSocket session을 유지하고, Controller가 그 session으로 즉시 task를 push한다"는 구조를 이해해야 한다.

## 기능 범위

### 1. README / 한국어 README 동기화

- [x] `README.md`를 persistent session 기준으로 업데이트한다.
- [x] `README.ko.md`를 같은 내용으로 동기화한다.
- [x] HTTP/WS는 테스트 전용, 제품/운영은 HTTPS/WSS라는 경고를 유지한다.

포함할 설명:

- Controller는 Agent로 직접 접속하지 않는다.
- Agent가 Controller에 outbound persistent WebSocket을 유지한다.
- connected Agent에서는 `Run`이 즉시 task를 push한다.
- offline Agent는 reconnect 후 queued job을 받는다.
- revoke는 active session close와 추가 task 차단을 의미한다.
- 이미 실행 중인 OS process kill은 별도 cancellation 기능이다.

### 2. Protocol/API/OpenAPI 문서 갱신

- [x] `docs/protocol.md`에 persistent session lifecycle을 반영한다.
- [x] `docs/api.md`에 job/session 상태 API를 반영한다.
- [x] `docs/openapi.json`에 신규/변경 endpoint와 schema를 반영한다.
- [x] `web-admin/api.schema.json`을 Web Admin 사용 API와 맞춘다.

Protocol 문서 필수 항목:

- auth 후 session registry 등록
- heartbeat는 liveness signal
- facts/metrics/log interval 분리
- task_assignment 즉시 push
- output_chunk/task_result streaming
- duplicate session 정책
- close reason 정책

API 문서 필수 항목:

- job detail 또는 dispatch_state 응답
- agent session summary 응답
- output API polling fallback
- token/private key/raw output 예시 금지

### 3. Smoke test 작성

- [x] local controller + agent + run immediate smoke script를 작성한다.
- [x] heartbeat interval을 30초로 둔 상태에서도 Run 직후 output이 관찰되는지 확인한다.
- [x] remote HTTP warning smoke를 유지한다.
- [x] HTTPS/WSS smoke를 유지한다.

Smoke 기준:

```text
controller start
agent start --heartbeat-interval-seconds 30
wait until session connected
create command job
assert task dispatched before next heartbeat interval
assert output observed
assert task_result success
```

## 테스트와 검증

필수:

- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `npm test --workspace web-admin`
- [x] `npm run typecheck --workspace web-admin`
- [x] `npm run build --workspace web-admin`
- [x] local immediate run smoke
- [x] remote HTTP warning smoke
- [x] HTTPS/WSS smoke
- [x] `git diff --check`

문서 검증:

- [x] README.md와 README.ko.md 명령 예시가 같은 흐름이다.
- [x] CLI help와 README 예시가 충돌하지 않는다.
- [x] Swagger/OpenAPI endpoint와 실제 route가 일치한다.
- [x] "Controller가 Agent로 접속한다"는 표현이 없다.

## 완료 기준

- [x] 사용자는 persistent outbound session 모델을 문서로 이해할 수 있다.
- [x] connected Agent에서 Run 결과가 즉시 나오는 것을 smoke로 확인한다.
- [x] HTTP 테스트 전용 경고 정책이 문서와 구현에 남아 있다.
- [x] Swagger/OpenAPI와 Web Admin API schema가 최신이다.
- [x] 전체 release 전 검증 게이트를 통과한다.

## 비범위

- [x] multi-controller HA 문서화하지 않음
- [x] full enterprise deployment guide 작성하지 않음
- [x] Ansible full compatibility 문서화하지 않음