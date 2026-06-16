# Task 001: 문서 최신화, 구현 Feature Matrix, Release Gate 정리

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 004
의존성: 없음
결과물: 현재 구현 기준선 문서, feature matrix, release gate

## 목표

현재 구현과 문서의 차이를 먼저 줄인다. 이 작업은 단순 문서 정리가 아니라 이후 phase에서 기준으로 삼을 "현재 무엇이 되고, 무엇이 부분 구현이며, 무엇이 아직 안 되는지"를 확정하는 작업이다.

사용자가 README만 보고 controller와 agent를 설치, 초기화, 연결, 확인, 삭제/재등록할 수 있어야 한다. 또한 개발자가 다음 기능을 추가할 때 어떤 검증 명령을 반드시 실행해야 하는지도 분명해야 한다.

## 기능 묶음

1. README/README.ko/docs/PROJECT 정책 동기화
2. 구현 feature matrix 작성
3. release gate와 stale docs scan 기준 확정

## 구현 체크리스트

문서 기준선:

- [x] README.md에서 Controller는 중앙 서버, Agent는 대상 서버라는 용어를 첫 부분에 명확히 설명한다.
- [x] README.ko.md도 README.md와 같은 구조와 의미로 맞춘다.
- [x] controller 하나에 여러 agent가 outbound로 붙는 구조를 명확히 쓴다.
- [x] controller가 agent로 직접 inbound 접속하지 않는다는 점을 명시한다.
- [x] HTTP는 사용 가능하지만 test-only라는 경고를 유지한다.
- [x] production 설명은 HTTPS 중심으로 정리한다.
- [x] HTTPS 준비 절차는 기본 흐름과 분리해서 설명한다.
- [x] `--dev-insecure-loopback` 잔여 예시를 제거하거나 과거 설명으로 격리한다.
- [x] agent 초기화는 현재 권장 UX인 `sponzey agent init` 중심으로 정리한다.
- [x] agent 삭제/재등록은 data dir 삭제, revoke, 새 token 발급의 차이를 구분한다.

세부 문서:

- [x] `docs/api.md`에서 구현된 endpoint와 미구현 endpoint를 구분한다.
- [x] facts/metrics/drift paging contract를 문서화한다.
- [x] `agent_system_time_ms`와 `stored_at`의 의미를 문서화한다.
- [x] `docs/logs.md`에서 log interval이 heartbeat와 독립된 현재 구조를 반영한다.
- [x] `docs/service-install.md`에서 `agent init` 중심 예시로 정리한다.
- [x] `docs/release-notes-mvp.md`를 v0.0.14 기준 현재 구현으로 갱신한다.
- [x] `npm/fleet/README.md`에서 지원 platform과 미지원 platform을 분리한다.
- [x] `PROJECT.md`의 HTTP 정책, persistent session, npm/package 설명이 현재 구현과 충돌하는지 확인한다.

Feature matrix:

- [x] `docs/feature-matrix.md` 또는 적절한 문서에 현재 기능 목록을 만든다.
- [x] 각 기능을 `Implemented`, `Partial`, `Planned`, `Policy decision required`로 표시한다.
- [x] Controller 기능을 정리한다.
- [x] Agent 기능을 정리한다.
- [x] Web Admin 기능을 정리한다.
- [x] CLI 기능을 정리한다.
- [x] npm/package 기능을 정리한다.
- [x] API/OpenAPI 기능을 정리한다.
- [x] security/audit 기능을 정리한다.

Release gate:

- [x] `cargo fmt --check`를 release gate에 포함한다.
- [x] `cargo clippy --workspace --all-targets -- -D warnings`를 release gate에 포함한다.
- [x] `cargo test --workspace`를 release gate에 포함한다.
- [x] `npm test --workspace @sponzey/fleet`를 release gate에 포함한다.
- [x] 현재 존재하는 smoke script 목록을 조사한다.
- [x] release 전 필수 smoke와 선택 smoke를 구분한다.
- [x] stale docs scan keyword 목록을 정리한다.

## 테스트

- [x] README 명령이 현재 CLI help와 충돌하지 않는지 수동 확인한다.
- [x] API 문서의 endpoint가 실제 route와 크게 어긋나지 않는지 확인한다.
- [x] stale keyword scan을 실행한다.
- [x] 영어/한글 README의 주요 명령과 흐름이 동일한지 비교한다.

## 검증 명령

```bash
rg -n --glob '!docs/release-gate.md' "dev-insecure-loopback|insecure remote HTTP|planned release package|sponzey agent enroll|agent enroll --" README.md README.ko.md PROJECT.md docs npm
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm test --workspace @sponzey/fleet
git diff --check
```

## 문서 업데이트

- [x] README.md
- [x] README.ko.md
- [x] PROJECT.md
- [x] docs/api.md
- [x] docs/logs.md
- [x] docs/service-install.md
- [x] docs/release-notes-mvp.md
- [x] npm/fleet/README.md
- [x] feature matrix 문서

## 완료 기준

- [x] 초보자도 문서만 보고 controller start, token 생성, agent init/start, Web Admin 확인까지 따라갈 수 있다.
- [x] HTTP와 HTTPS 설명이 중복 나열이 아니라 기본 흐름과 HTTPS 준비로 분리되어 있다.
- [x] 주요 문서에 현재 없는 CLI 옵션이 남아 있지 않다.
- [x] 구현됨/부분 구현/미구현/정책 결정 필요 상태가 feature matrix에 드러난다.
- [x] release 전에 반드시 실행할 검증 명령이 명확하다.
