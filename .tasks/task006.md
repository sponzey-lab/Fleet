# Task 006: Selector Preview와 Target Snapshot

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 006
의존성: Task 003, Task 004
결과물: multi-agent 실행 전 대상 확정 모델

## 목표

운영자가 실행 전에 어떤 agent가 대상인지 확인할 수 있게 하고, job 생성 이후 labels나 agent 상태가 바뀌어도 실행 대상이 흔들리지 않도록 target snapshot을 저장한다.

## 기능 묶음

1. selector parser 정리
2. selector preview API
3. target snapshot persistence

## 구현 체크리스트

Selector:

- [x] 현재 selector 지원 범위를 조사한다.
- [x] `agent:<name-or-id>` selector 의미를 고정한다.
- [x] `label:key=value` selector 의미를 고정한다.
- [x] `matchLabels` object selector를 설계한다.
- [x] group selector는 이번 task에서 제외하고 planned 상태로 문서화한다.
- [x] query selector는 이번 task에서 제외하고 planned 상태로 문서화한다.
- [x] selector parser를 domain/application 경계로 이동하거나 정리한다.
- [x] invalid selector error를 사용자 친화적으로 만든다.

Preview API:

- [x] selector preview request DTO를 설계한다.
- [x] preview 결과에 agent id/name/labels/status를 포함한다.
- [x] revoked/disabled/offline agent 포함 여부 정책을 정한다.
- [x] preview 결과에 warning을 포함할지 결정한다.
- [x] preview API auth를 적용한다.
- [x] preview API audit 필요 여부를 결정한다.

Target Snapshot:

- [x] job 생성 시 selector string/object를 저장한다.
- [x] selector result snapshot을 저장한다.
- [x] target agent id와 display name을 저장한다.
- [x] target labels snapshot 필요 여부를 정한다.
- [x] labels 변경 후 기존 job target이 바뀌지 않게 한다.
- [x] snapshot이 assignment 생성의 source of truth가 되도록 한다.

## 테스트

- [x] `agent:<name>` selector matching test
- [x] `agent:<id>` selector matching test
- [x] `label:key=value` selector matching test
- [x] `matchLabels` selector matching test
- [x] invalid selector rejection test
- [x] selector preview API test
- [x] job target snapshot persistence test
- [x] labels changed after job creation test
- [x] revoked/offline target policy test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 selector preview API를 문서화한다.
- [x] README 또는 docs에 selector 예시를 추가한다.
- [x] Web Admin target preview UX 계획을 업데이트한다.

## 완료 기준

- [x] 실행 전에 대상 agent를 preview할 수 있다.
- [x] job 생성 후 target snapshot이 고정된다.
- [x] labels 변경이 기존 job target을 바꾸지 않는다.
- [x] selector parser가 테스트로 고정된다.
