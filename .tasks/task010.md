# Task 010: Runbook Schema와 Result Model

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 008
의존성: Task 006, Task 008
결과물: runbook DSL과 primitive result의 안정된 계약

## 목표

runbook primitive를 늘리기 전에 schema와 result model을 먼저 고정한다. 자동화 제품에서 중요한 것은 실행 여부뿐 아니라 changed/skipped/failed, diff, duration, target별 결과를 일관되게 해석할 수 있는 것이다.

## 기능 묶음

1. runbook schema version과 parser contract
2. strategy/check mode/dry-run model
3. primitive result common schema

## 구현 체크리스트

Schema:

- [x] 현재 runbook parser와 schema를 조사한다.
- [x] schema version 필드를 필수화할지 결정한다.
- [x] `name`, `description`, `selector`, `strategy`, `steps` 구조를 정리한다.
- [x] `matchLabels` selector와 runbook selector의 연결을 정리한다.
- [x] invalid YAML error를 사용자 친화적으로 만든다.
- [x] unknown field handling 정책을 정한다.
- [x] backward compatibility fixture를 만든다.

Strategy:

- [x] `strategy.concurrency`를 schema에 반영한다.
- [x] `strategy.maxFailures`를 schema에 반영한다.
- [x] check mode를 설계한다.
- [x] dry-run과 check mode의 차이를 정한다.
- [x] approval required 판정이 runbook 전체/step 단위 중 어디에 적용되는지 정한다.

Result Model:

- [x] primitive common result schema를 정의한다.
- [x] status: changed/skipped/success/failed/rejected/canceled 등을 정리한다.
- [x] changed boolean 또는 status 표현 방식을 결정한다.
- [x] diff field 구조를 정한다.
- [x] message field를 정한다.
- [x] started_at/completed_at/duration_ms를 정한다.
- [x] stdout/stderr와 product log 분리를 유지한다.
- [x] per-step result와 assignment result aggregate 규칙을 정한다.

## 테스트

- [x] valid runbook fixture test
- [x] invalid YAML fixture test
- [x] unknown field policy test
- [x] backward compatibility fixture test
- [x] strategy parsing test
- [x] check mode parsing test
- [x] dry-run parsing test
- [x] primitive result serialization test
- [x] per-step aggregate result test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/runbooks.md 또는 runbook 섹션을 추가한다.
- [x] runbook YAML 예시를 추가한다.
- [x] result schema 예시를 추가한다.
- [x] dry-run/check mode 의미를 문서화한다.

## 완료 기준

- [x] runbook schema가 fixture test로 고정된다.
- [x] primitive result common schema가 명확하다.
- [x] strategy와 selector가 job/fanout model과 충돌하지 않는다.
- [x] safe primitive 확장을 시작할 수 있다.

## 구현 결과

- Canonical runbook schema를 `apiVersion`, `kind`, `name`, `description`, `selector`/`matchLabels`, `strategy`, `checkMode`, `dryRun`, `steps`로 정리했다.
- 기존 `metadata/spec/targets/tasks` 구조는 `examples/runbooks/legacy-nginx-basic.yml` fixture로 유지했다.
- Parser는 unknown top-level/spec/task field를 거부하고, invalid YAML 메시지를 `expected key: value` 형태로 더 명확히 반환한다.
- `strategy.concurrency`, `strategy.maxFailures`, `checkMode`, `dryRun` parsing을 domain test로 고정했다.
- Controller runbook job은 request target 지정이 없을 때 runbook 문서 selector를 사용해 target snapshot을 만든다.
- Runner는 `dryRun`이면 모든 primitive를 `skipped`, `checkMode`이면 low-risk check만 실행하고 mutation step을 `skipped`로 처리한다.
- Primitive result common schema에 `status`, `changed`, `message`, `diff`, `started_at_ms`, `completed_at_ms`, `duration_ms`를 추가하고 serialization test를 추가했다.
- Per-step aggregate rule은 `canceled > rejected > failed > changed > skipped > success` 순서로 고정했다.
- `docs/runbooks.md`, `docs/api.md`, `docs/openapi.json`, `docs/feature-matrix.md`를 현재 계약에 맞춰 갱신했다.

## 검증 결과

- [x] `cargo fmt --all`
- [x] `cargo test -p fleet-domain -p fleet-runner -p fleet-controller`
- [x] `cargo fmt --all --check`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `git diff --check`
