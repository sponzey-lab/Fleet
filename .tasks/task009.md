# Task 009: Admin Auth/RBAC 초안과 Permission Check

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 007
의존성: Task 008
결과물: admin token 이후 권한 모델의 최소 기반

## 목표

현재 admin token 중심 접근을 제품형 admin identity와 permission check로 확장할 수 있는 기반을 만든다. 이번 task는 full OIDC/RBAC 구현이 아니라, approval과 위험 작업을 안전하게 다루기 위한 최소 권한 경계다.

## 기능 묶음

1. Admin identity/session model 초안
2. Role/permission matrix 초안
3. API permission check 적용 지점 정리

## 구현 체크리스트

Admin Identity:

- [x] 현재 admin token 인증 경로를 조사한다.
- [x] bootstrap admin token과 product admin identity의 관계를 정한다.
- [x] admin actor id 개념을 추가한다.
- [x] API request context에 actor를 명시적으로 전달한다.
- [x] audit event에 actor를 연결한다.
- [x] CLI profile/login과의 관계를 정리한다.

RBAC 초안:

- [x] role 후보를 정한다: owner/admin/operator/viewer.
- [x] permission 후보를 정한다.
- [x] agent read permission
- [x] job create permission
- [x] job approve permission
- [x] job cancel permission
- [x] enrollment token create permission
- [x] agent revoke permission
- [x] audit read permission
- [x] policy write permission
- [x] permission matrix 문서를 작성한다.

Permission Check:

- [x] API handler에서 permission check가 들어갈 공통 경계를 정한다.
- [x] UI는 권한을 결정하지 않음을 확인한다.
- [x] forbidden response model을 정한다.
- [x] approval approve/reject에 permission check를 적용한다.
- [x] enrollment token creation에 permission check를 적용한다.
- [x] agent revoke에 permission check를 적용한다.

## 테스트

- [x] admin token maps to bootstrap actor test
- [x] permission allowed test
- [x] permission denied test
- [x] approval approve requires permission test
- [x] enrollment token create requires permission test
- [x] agent revoke requires permission test
- [x] forbidden response contract test
- [x] audit includes actor test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

검증 결과:

- [x] `cargo fmt --all --check`
- [x] `cargo test -p fleet-application -p fleet-store -p fleet-controller`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `node web-admin/scripts/test.js`
- [x] `git diff --check`

## 문서 업데이트

- [x] docs/security.md 또는 README에 admin token과 향후 admin model 관계를 설명한다.
- [x] docs/api.md에 401/403 response 차이를 문서화한다.
- [x] Web Admin forbidden message 기준을 정리한다.

## 완료 기준

- [x] admin 요청에는 actor 개념이 붙는다.
- [x] 위험 API에는 permission check 경계가 있다.
- [x] UI가 권한 판단을 하지 않는다.
- [x] full OIDC/RBAC는 later로 남기되 확장 경로가 막히지 않는다.

## 구현 결과

- Bootstrap admin token은 `bootstrap-admin` actor와 `owner` role로 인증된다.
- Controller protected API 경계에서 admin token을 `AdminRequestContext`로 변환하고 route별 permission을 검사한다.
- `owner`/`admin`은 전체 권한, `operator`는 job/approval 중심 권한, `viewer`는 조회 중심 권한으로 정의했다.
- Approval approve/reject, enrollment token create/revoke, agent revoke, job create/cancel, selector preview 등에 permission check를 적용했다.
- Enrollment token 생성/폐기, agent label/revoke, approval approve/reject, job 생성 audit은 인증된 admin actor를 사용한다.
- Approval decision request의 legacy `actor` field는 호환을 위해 허용하지만 audit/authorization에는 사용하지 않는다.
- Web Admin은 401/403을 권한 문제로 안내하지만 권한 판단은 controller가 한다.
- `docs/security.md`, `docs/api.md`, `docs/openapi.json`, README를 현재 인증/권한 모델에 맞춰 갱신했다.
