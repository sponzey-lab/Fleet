# Task 007: Fanout Concurrency, MaxFailures, Partial Success

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 006
의존성: Task 006
결과물: 여러 agent 대상 실행 모델

## 목표

하나의 job을 여러 agent에 안전하게 분배한다. target snapshot을 기준으로 assignment를 만들고, concurrency와 maxFailures를 지키며, 전체 결과를 success/failed/partial_success로 계산한다.

## 기능 묶음

1. fanout dispatcher
2. concurrency/maxFailures 정책
3. target별 summary와 aggregate result

## 구현 체크리스트

Fanout:

- [x] job target snapshot에서 assignment를 생성한다.
- [x] 각 target agent마다 assignment를 하나씩 만든다.
- [x] disconnected target 처리 정책을 구현한다.
- [x] revoked/disabled target 처리 정책을 구현한다.
- [x] assignment dispatch queue를 만든다.
- [x] active session이 있으면 즉시 dispatch한다.
- [x] active session이 없으면 queued 또는 unreachable 정책에 따른다.

Concurrency/MaxFailures:

- [x] job strategy에 concurrency 필드를 추가한다.
- [x] concurrency 기본값을 정한다.
- [x] concurrency가 1일 때 순차 실행을 보장한다.
- [x] concurrency가 N일 때 동시에 N개 이하만 dispatch한다.
- [x] maxFailures 필드를 추가한다.
- [x] maxFailures 도달 시 남은 queued assignment를 중단한다.
- [x] maxFailures 도달 audit/log를 남긴다.

Result Summary:

- [x] target별 assignment summary API를 만든다.
- [x] job aggregate status 계산을 적용한다.
- [x] partial_success 계산을 구현한다.
- [x] skipped/canceled/expired target 수를 summary에 포함한다.
- [x] Web Admin에서 target별 결과를 표시할 수 있는 response를 만든다.

## 테스트

- [x] fanout creates assignment per target test
- [x] concurrency one runs sequentially test
- [x] concurrency N limit test
- [x] maxFailures stops queued assignments test
- [x] offline target summary test
- [x] revoked target summary test
- [x] all success aggregate success test
- [x] all failed aggregate failed test
- [x] mixed result aggregate partial_success test
- [x] Web Admin API response shape test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 fanout job request/response를 문서화한다.
- [x] runbook strategy 문서에 concurrency/maxFailures를 추가한다.
- [x] README에 multi-agent 실행은 target preview 후 실행하는 흐름이라고 설명한다.

## 완료 기준

- [x] target snapshot 기준으로 multi-agent job이 실행된다.
- [x] concurrency 제한이 테스트로 보장된다.
- [x] maxFailures 도달 시 남은 assignment가 무작정 실행되지 않는다.
- [x] partial_success가 API/UI에서 설명 가능하다.
