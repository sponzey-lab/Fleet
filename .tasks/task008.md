# Task 008: Approval Request Lifecycle

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 007
의존성: Task 003, Task 006
결과물: 위험 작업 승인 workflow

## 목표

`confirmed_high_risk` 같은 단순 확인을 제품형 approval lifecycle로 전환한다. 위험 작업은 approval request 없이 실행되지 않아야 하며, 승인자와 승인 사유가 audit로 남아야 한다.

## 기능 묶음

1. ApprovalRequest domain entity
2. 위험 작업 분류와 approval required 판정
3. approve/reject/expire workflow

## 구현 체크리스트

Domain:

- [x] `ApprovalRequest` entity를 설계한다.
- [x] approval id, job id, requester, approver, reason, status, expiry를 정의한다.
- [x] status는 pending/approved/rejected/expired/canceled를 포함한다.
- [x] approval request 생성 use case를 만든다.
- [x] approve use case를 만든다.
- [x] reject use case를 만든다.
- [x] expiry 처리 정책을 만든다.

Risk Classification:

- [x] high-risk command 기준을 정리한다.
- [x] shell primitive는 high-risk로 분류한다.
- [x] reboot primitive는 approval required로 분류한다.
- [x] user/group primitive는 approval required로 분류한다.
- [x] broad target selector 기준을 정한다.
- [x] root-required task 기준을 정한다.
- [x] approval required 판정을 domain/application test로 고정한다.

Dispatch Integration:

- [x] approval required job은 pending_approval 상태로 만든다.
- [x] approval 전에는 assignment를 dispatch하지 않는다.
- [x] approval 후 queued 상태로 전환한다.
- [x] reject된 job은 dispatch할 수 없다.
- [x] expired approval은 dispatch할 수 없다.
- [x] 기존 `confirmed_high_risk` 경로를 compatibility shim으로 둘지 제거할지 결정한다.

Audit:

- [x] approval requested audit event를 남긴다.
- [x] approval approved audit event를 남긴다.
- [x] approval rejected audit event를 남긴다.
- [x] approval expired audit event를 남긴다.
- [x] approver identity를 audit와 연결한다.

## 테스트

- [x] approval request creation test
- [x] high-risk classification test
- [x] safe command does not require approval test
- [x] broad selector requires approval test
- [x] pending approval does not dispatch test
- [x] approved job dispatches test
- [x] rejected job cannot dispatch test
- [x] expired approval cannot dispatch test
- [x] audit event creation test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 approval endpoint를 문서화한다.
- [x] README에 위험 작업은 approval이 필요하다는 설명을 추가한다.
- [x] Web Admin approval queue task와 연결되는 UI 요구사항을 정리한다.

## 완료 기준

- [x] 위험 작업은 approval 없이 dispatch되지 않는다.
- [x] 승인/거절/만료 상태가 domain test로 고정된다.
- [x] approval과 job dispatch가 audit로 연결된다.
- [x] 기존 high-risk 확인 경로의 호환 정책이 명확하다.

## 구현 결과

- `fleet-domain`에 `ApprovalRequest`, `ApprovalStatus`, approval id, approval transition을 추가했다.
- command/runbook/drift job 생성 use case가 approval requirement를 계산하고, 필요한 경우 `pending_approval` job과 approval request를 생성한다.
- `confirmed_high_risk`는 compatibility acknowledgement로 유지하되 approval을 대체하지 않는다.
- Controller API에 `GET /api/approvals`, `POST /api/approvals/{approval_id}/approve`, `POST /api/approvals/{approval_id}/reject`, `POST /api/approvals/expire`를 추가했다.
- Approval approve 이후 linked job은 queued로 전환되며 active agent session이 있으면 즉시 dispatch를 시도한다.
- Rejected approval은 linked job을 failed로, expired approval은 linked job을 expired로 전환한다.
- Web Admin shared API client/schema와 OpenAPI 문서를 approval API와 동기화했다.

## 검증 결과

- [x] `cargo fmt --all`
- [x] `cargo test -p fleet-application -p fleet-controller -p fleet-domain -p fleet-store`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `node web-admin/scripts/test.js`
- [x] `node web-admin/scripts/typecheck.js`
- [x] `docs/openapi.json`, `web-admin/api.schema.json` JSON parse
