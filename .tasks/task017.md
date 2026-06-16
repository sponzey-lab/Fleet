# Task 017: Web Admin Product UX

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 014
의존성: Task 003, Task 006, Task 008, Task 013 권장
결과물: 운영자가 혼동하지 않는 얇은 Web Admin UI

## 목표

Web Admin을 개발 확인용 화면에서 반복 사용 가능한 운영 화면으로 다듬는다. UI는 권한과 domain rule을 결정하지 않고, controller API 결과를 정확하고 안전하게 표시한다.

## 기능 묶음

1. agent/job 상태 가시성
2. approval/runbook/policy 화면
3. telemetry와 error UX 정리

## 구현 체크리스트

Agent/Job 상태:

- [x] agent list status badge를 정리한다.
- [x] online/offline/stale/revoked 상태를 명확히 표시한다.
- [x] revoked agent가 online처럼 보이지 않게 한다.
- [x] selected agent 변경 regression을 테스트한다.
- [x] agent detail refresh UX를 정리한다.
- [x] job과 assignment 상태를 구분해서 표시한다.
- [x] target별 assignment table을 추가한다.
- [x] job output loading/no output/completed 상태를 구분한다.

Approval/Runbook/Policy:

- [x] approval queue 화면을 추가한다.
- [x] approval detail에서 위험 이유와 target snapshot을 보여준다.
- [x] approve/reject action은 controller API 결과를 따른다.
- [x] runbook catalog 또는 upload/validate 화면을 설계한다.
- [x] runbook dry-run result를 표시한다.
- [x] policy list 화면을 설계한다.
- [x] policy assignment 화면을 설계한다.
- [x] drift history와 remediation action을 표시한다.

Telemetry/Error UX:

- [x] facts inventory grouping을 정리한다.
- [x] disk/mount inventory table을 추가한다.
- [x] metrics range selector를 추가한다.
- [x] metrics chart refresh를 명확히 한다.
- [x] log viewer를 설계한다.
- [x] HTTP transport warning banner를 표시한다.
- [x] 401/403/404/409 error message를 구분한다.
- [x] admin auth expired 상태를 처리한다.
- [x] favicon/static asset completeness를 확인한다.

## 테스트

- [x] API client unit test
- [x] agent list rendering test
- [x] selected agent switching test
- [x] revoked agent display test
- [x] job output rendering test
- [x] target assignment table rendering test
- [x] metrics chart rendering smoke
- [x] facts inventory rendering test
- [x] approval queue rendering test
- [x] UI build test

## 검증 명령

```bash
npm test --workspace @sponzey/fleet
cargo test --workspace
git diff --check
```

web-admin build 명령이 별도이면 release gate에 맞춰 추가한다.

## 문서 업데이트

- [x] README Web Admin 설명을 최신화한다.
- [x] README.ko.md Web Admin 설명을 동기화한다.
- [x] docs/api.md의 UI 사용 API와 실제 화면 흐름이 충돌하지 않는지 확인한다.
- [x] screenshots나 UI 설명 문서가 있으면 최신화한다.

## 완료 기준

- [x] 운영자가 agent와 job 상태를 혼동하지 않는다.
- [x] revoked/offline/stale 상태가 명확히 보인다.
- [x] 위험 작업은 approval 흐름으로 보인다.
- [x] facts와 metrics 의미가 화면에서 구분된다.
- [x] UI가 domain rule이나 authorization을 자체 판단하지 않는다.

## 구현 결과

- Agent list/detail은 online/offline/stale/revoked, session connected/disconnected, assigned policy를 분리해서 표시한다.
- Facts는 inventory grid와 disk/mount table을 분리하고, Metrics는 range selector 기반 chart를 제공한다.
- Drift는 latest와 history를 함께 보여주며, agent operational log viewer를 추가했다.
- Job output viewer는 pending approval, queued, running, completed no-output, failed/rejected/canceled/expired 상태를 구분하고, target별 assignment table을 별도 표시한다.
- Approval queue는 pending approval을 표시하고 approve/reject/expire due action을 controller API로 수행한다.
- Runbook 화면은 YAML 입력으로 signed runbook job을 만들고 status/result를 표시한다. 별도 runbook catalog 저장소와 validate-only endpoint는 아직 없으므로 후속 제품화 범위다.
- Policy 화면은 source 저장, list, selected agent assignment, drift schedule action을 제공한다.
- HTTP 접속 시 화면 상단 warning banner를 표시하며, 401/403/404/409 error message를 구분한다.

## 검증 결과

- [x] `npm test --workspace web-admin`
- [x] `npm run typecheck --workspace web-admin`
- [x] `npm run build --workspace web-admin`
- [x] `cargo fmt --check`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `git diff --check`