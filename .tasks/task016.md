# Task 016: OpenAPI/SDK Contract

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 013
의존성: Task 003, Task 006, Task 008 권장
결과물: public API 계약과 Swagger 정합성

## 목표

외부 시스템이 Sponzey Fleet를 안정적으로 제어할 수 있도록 public API와 internal/protocol API를 구분하고, OpenAPI 문서를 실제 구현과 맞춘다.

## 기능 묶음

1. endpoint inventory와 public/internal 구분
2. common response/error/pagination contract
3. OpenAPI snapshot과 client 생성 준비

## 구현 체크리스트

Endpoint Inventory:

- [x] 모든 HTTP route를 조사한다.
- [x] admin API를 구분한다.
- [x] agent protocol API를 구분한다.
- [x] public stable API 후보를 구분한다.
- [x] internal/unstable API를 구분한다.
- [x] deprecated 또는 planned endpoint를 문서에서 분리한다.

Common Contract:

- [x] pagination 공통 request/response 모델을 정리한다.
- [x] error response 공통 모델을 정리한다.
- [x] auth error 401과 permission error 403을 구분한다.
- [x] not_found 404와 empty latest result의 차이를 정리한다.
- [x] conflict 409 사용 기준을 정한다.
- [x] job/assignment response model을 정리한다.
- [x] facts/metrics/drift page response model을 정리한다.

OpenAPI/SDK:

- [x] OpenAPI schema generation 경로를 확인한다.
- [x] Swagger UI 접근 경로를 확인한다.
- [x] OpenAPI example payload를 추가한다.
- [x] OpenAPI schema snapshot test를 검토한다.
- [x] TypeScript generated client 생성 여부를 결정한다.
- [x] Rust client crate 생성 여부를 결정한다.
- [x] CLI가 public API client를 재사용할 수 있는지 검토한다.

## 테스트

- [x] OpenAPI schema snapshot test
- [x] endpoint coverage test
- [x] pagination contract test
- [x] error response contract test
- [x] auth/permission response test
- [x] facts page response test
- [x] metrics page response test
- [x] drift page response test
- [x] generated client smoke if added

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md를 OpenAPI와 동기화한다.
- [x] README에 Swagger 접근 경로를 명확히 둔다.
- [x] API compatibility policy를 문서화한다.
- [x] deprecation policy를 문서화한다.

## 완료 기준

- [x] Swagger를 보고 주요 API 흐름을 이해할 수 있다.
- [x] public/internal endpoint 경계가 명확하다.
- [x] pagination과 error response가 일관된다.
- [x] breaking change를 snapshot/contract test로 감지할 수 있다.

## 구현 결과

- Controller test에 REST API route contract를 추가하여 public/admin/agent protocol endpoint가 OpenAPI에 빠지면 실패하도록 했다.
- Admin endpoint는 OpenAPI operation에서 `bearerAuth`를 요구하고, public readiness/docs와 enrollment endpoint는 admin bearer auth를 요구하지 않도록 검증한다.
- `/api/agents/ws`와 `/admin/*`는 REST API가 아니라는 것을 test와 문서에 고정했다.
- latest facts/metrics/drift의 데이터 없음은 `200 null`, 명시 resource 없음은 `404 not_found`로 구분하는 regression test를 추가했다.
- Web Admin API client smoke가 policy, assignment, schedule, scheduled drift endpoint까지 호출/encoding을 검증한다.
- `docs/api.md`, README 영문/한글, feature matrix, plan을 API surface, common contract, compatibility/deprecation 기준에 맞춰 갱신했다.
- TypeScript generated SDK와 Rust client crate는 이번 단계에서 생성하지 않고, Web Admin dependency-free client와 API schema를 최소 client contract로 유지하기로 결정했다.

## 검증 결과

- [x] `cargo fmt --check`
- [x] `cargo test -p fleet-controller openapi`
- [x] `cargo test -p fleet-controller latest_optional_resources_return_null_instead_of_not_found`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `npm test --workspace web-admin`
- [x] `npm run typecheck --workspace web-admin`
- [x] `git diff --check`
