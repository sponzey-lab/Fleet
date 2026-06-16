# Task 012: Policy Assignment와 Scheduled Drift

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 009
의존성: Task 006, Task 008, Task 010
결과물: policy 기반 drift 운영 루프

## 목표

drift detection을 단발성 확인에서 policy 기반 운영 루프로 확장한다. 어떤 agent가 어떤 policy를 따라야 하는지 기록하고, scheduled drift check와 remediation approval로 연결한다.

## 기능 묶음

1. Policy object와 assignment
2. scheduled drift check
3. remediation approval 연결

## 구현 체크리스트

Policy:

- [x] `Policy` domain entity를 설계한다.
- [x] policy id/name/version/source를 정의한다.
- [x] policy validation을 만든다.
- [x] policy source 저장 방식을 정한다.
- [x] policy assignment 대상이 agent/group/selector 중 어디까지인지 이번 scope를 정한다.
- [x] agent inventory/API에 policy_id 또는 assigned policy 정보를 반영한다.

Scheduled Drift:

- [x] drift check schedule model을 설계한다.
- [x] fake clock으로 테스트 가능한 scheduler 경계를 만든다.
- [x] missed schedule handling 정책을 정한다.
- [x] latest drift와 drift history를 구분한다.
- [x] drift severity model을 만든다.
- [x] drift acknowledged state를 만든다.
- [x] scheduled drift result audit를 남긴다.

Remediation:

- [x] policy에 remediation runbook reference를 연결할지 결정한다.
- [x] drift에서 remediation request를 생성한다.
- [x] remediation은 approval required로 연결한다.
- [x] remediation result와 drift resolution 연결 정책을 정한다.
- [x] 자동 remediation은 non-goal로 유지한다.

## 테스트

- [x] policy validation unit test
- [x] policy assignment application test
- [x] scheduled drift fake clock test
- [x] missed schedule handling test
- [x] drift history paging test
- [x] drift acknowledged state test
- [x] remediation approval required test
- [x] remediation result updates drift state test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/policy.md 또는 policy 섹션을 추가한다.
- [x] drift latest와 history의 차이를 문서화한다.
- [x] remediation은 자동 실행이 아니라 approval workflow를 탄다고 명시한다.

## 완료 기준

- [x] agent와 policy의 연결을 API로 확인할 수 있다.
- [x] drift가 history로 추적된다.
- [x] scheduled drift는 fake clock 테스트가 있다.
- [x] remediation은 approval 없이 실행되지 않는다.

## 구현 메모

- 이번 scope의 assignment는 direct agent assignment까지 구현한다. Selector/group rollout worker와 Web Admin policy 화면은 `.tasks/plan.md` Phase 009 후속 항목으로 남긴다.
- Scheduled drift는 domain/application/store/API 경계를 구현했다. Background worker가 due schedule을 읽어 drift-check job을 생성하는 루프는 후속 항목이다.
- Drift report는 severity와 acknowledgement/resolution 상태를 저장하고 API로 반환한다.