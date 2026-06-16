# Task 005: Cancel/Timeout/Reconnect 복구

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 005
의존성: Task 004
결과물: 끊김과 취소 상황에서 신뢰 가능한 assignment 처리

## 목표

agent disconnect, controller restart, task timeout, operator cancel 상황에서 job/assignment 상태가 일관되게 남도록 만든다. 원격 실행 제품에서 "끊겼는데 성공처럼 보이는 상태"를 허용하지 않는다.

## 기능 묶음

1. cancel protocol과 process boundary
2. timeout 처리
3. reconnect/recovery 정책

## 구현 체크리스트

Cancel:

- [x] cancel API 또는 기존 cancel 경로를 확인한다.
- [x] cancel request domain use case를 정리한다.
- [x] cancel message protocol을 설계한다.
- [x] queued assignment cancel 처리
- [x] dispatched but not started assignment cancel 처리
- [x] running assignment cancel 처리
- [x] agent process runner kill/terminate 정책 정리
- [x] cancel result와 failed result를 구분한다.
- [x] cancel audit event를 남긴다.

Timeout:

- [x] task timeout 기본값을 확인한다.
- [x] timeout이 어디에서 결정되는지 확인한다.
- [x] assignment timeout deadline 저장 여부를 결정한다.
- [x] timeout 시 agent process 종료 정책을 구현한다.
- [x] timeout result와 canceled result를 구분한다.
- [x] timeout audit/log를 남긴다.

Reconnect/Recovery:

- [x] agent disconnect 시 active assignment 상태 처리 정책을 정한다.
- [x] reconnect 시 agent가 in-flight task 상태를 보고할지 결정한다.
- [x] controller restart 후 running assignment 복구 정책을 정한다.
- [x] stale dispatched assignment expiry 정책을 정한다.
- [x] duplicate session 발생 시 running task 처리 정책을 정한다.
- [x] recovery path가 active session registry에만 의존하지 않도록 한다.

## 테스트

- [x] cancel before dispatch test
- [x] cancel after dispatch before start test
- [x] cancel while running test
- [x] timeout while running test
- [x] disconnect while dispatched test
- [x] disconnect while running test
- [x] reconnect reports assignment state test
- [x] controller restart recovery policy test
- [x] duplicate session does not mark job success test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 cancel/timeout 상태를 문서화한다.
- [x] README troubleshooting에 agent disconnect/reconnect 기대 동작을 정리한다.
- [x] Web Admin 상태 표시 문구를 업데이트한다.

## 완료 기준

- [x] cancel과 timeout이 서로 다른 terminal result로 남는다.
- [x] disconnect/reconnect 상황에서 success로 오인하지 않는다.
- [x] process runner가 cancel/timeout 시 child process를 정리한다.
- [x] reconnect/recovery 정책이 테스트로 고정된다.

## 구현 결과

- `POST /api/jobs/{job_id}/cancel` API를 추가했다.
- cancel request body는 optional이며, `reason`이 있으면 audit와 assignment `last_error`에 redaction 후 반영한다.
- `WirePayload::TaskCancel`을 추가해 controller가 active agent session으로 cancel을 보낼 수 있게 했다.
- `TaskResultStatus`를 추가해 `succeeded`, `failed`, `canceled`, `timed_out`을 구분한다.
- 구버전 agent가 `status` 없이 `task_result`를 보내면 기존처럼 `exit_code` 기반으로 success/failed fallback 처리한다.
- `queued` assignment cancel은 DB에서 바로 `canceled` terminal 상태가 된다.
- `dispatched`, `accepted`, `started` assignment cancel은 DB 상태를 `canceled`로 만들고, active session이 있으면 `task_cancel`을 best-effort로 보낸다.
- agent persistent session은 현재 task id와 cancel flag를 가진 작은 runtime state를 유지한다.
- command task는 `run_command_streaming_with_cancel`을 사용해 cancel/timeout 시 child process를 kill한다.
- cancel result는 `canceled`, timeout result는 `timed_out`으로 controller에 보고된다.
- controller는 `timed_out`을 assignment/job `expired`로 저장한다.
- 이미 terminal 상태인 assignment는 늦은 `task_result`로 덮어쓰지 않는다. cancel 후 late success가 와도 job은 `canceled`로 남는다.
- reconnect 시 별도 in-flight report payload는 추가하지 않았다. 현재 정책은 DB의 assignment 상태를 source of truth로 두고, queued만 reconnect drain 대상이며, dispatched/started terminal 결정은 result/cancel/timeout/expiry/reconciler가 담당한다.
- controller restart 후 active session registry는 비어 있을 수 있으므로 recovery 판단은 registry만 보지 않고 DB assignment 상태를 기준으로 한다.

## 검증 결과

- `cargo fmt --check`: 통과
- `cargo test --workspace`: 통과
- `cargo clippy --workspace --all-targets -- -D warnings`: 통과
- `git diff --check`: 통과
- `node web-admin/scripts/test.js`: 통과
- `node web-admin/scripts/typecheck.js`: 통과
- `docs/openapi.json`, `web-admin/api.schema.json` JSON parse: 통과
