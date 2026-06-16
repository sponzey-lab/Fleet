# Task 011: Safe Primitive 확장

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 008
의존성: Task 010
결과물: 승인 없이 확장 가능한 안전 primitive

## 목표

approval/capability 없이 위험한 primitive를 열지 않는다. 먼저 idempotency와 side effect가 비교적 통제 가능한 safe primitive를 확장하고, result model을 실제 실행 경로에 적용한다.

## 기능 묶음

1. existing package/service/file.copy idempotency 정리
2. check primitive 추가
3. facts/metrics/log snapshot primitive 정리

## 구현 체크리스트

기존 primitive:

- [x] command primitive result를 common result schema에 맞춘다.
- [x] package primitive changed/skipped 판정을 강화한다.
- [x] service primitive changed/skipped 판정을 강화한다.
- [x] file.copy checksum/diff 판정을 강화한다.
- [x] primitive timeout/output limit을 일관화한다.
- [x] dry-run/check mode에서 side effect가 발생하지 않도록 한다.

Check primitive:

- [x] `port.check` primitive를 설계한다.
- [x] `process.check` primitive를 설계한다.
- [x] check primitive는 기본적으로 changed=false로 처리한다.
- [x] check 실패와 task 실패의 차이를 정한다.
- [x] result message를 operator가 이해할 수 있게 만든다.

Snapshot primitive:

- [x] `facts.collect` primitive를 설계한다.
- [x] `metrics.snapshot` primitive를 설계한다.
- [x] `logs.tail` primitive는 원문 output과 retention 정책을 고려해 scope를 제한한다.
- [x] snapshot primitive가 periodic collector와 충돌하지 않게 한다.

제외 대상:

- [x] `shell`은 이번 task에서 구현하지 않는다.
- [x] `reboot`은 이번 task에서 구현하지 않는다.
- [x] `user/group/cron`은 이번 task에서 구현하지 않는다.
- [x] 위험 primitive는 approval/capability task 이후로 둔다.

## 테스트

- [x] package idempotency test
- [x] service idempotency test
- [x] file.copy checksum no-change test
- [x] file.copy changed diff test
- [x] check mode no side effect test
- [x] port.check success/failure test
- [x] process.check success/failure test
- [x] facts.collect result schema test
- [x] metrics.snapshot result schema test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

검증 결과:

- [x] `cargo fmt --all --check`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `git diff --check`

## 문서 업데이트

- [x] runbook primitive reference 문서를 업데이트한다.
- [x] safe primitive와 dangerous primitive 구분을 문서화한다.
- [x] 각 primitive result 예시를 추가한다.

## 완료 기준

- [x] safe primitive는 common result schema를 따른다.
- [x] dry-run/check mode에서 side effect가 없다.
- [x] changed/skipped/failed 의미가 테스트로 고정된다.
- [x] 위험 primitive가 우회적으로 열리지 않는다.
