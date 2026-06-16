# Sponzey Fleet Feature Matrix

작성일: 2026-06-15
기준 구현: v0.0.15 릴리스 후보 현재 워킹트리

상태 표기:

- `Implemented`: 현재 동작하고 문서/테스트가 대체로 존재한다.
- `Partial`: 기본 흐름은 있으나 제품화에 필요한 상태 모델, UX, 테스트, 문서가 더 필요하다.
- `Planned`: 계획은 있으나 아직 구현되지 않았다.
- `Policy decision required`: 기술 구현 전에 제품 정책 결정이 필요하다.

## Controller

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| 단일 `sponzey` 바이너리의 controller subcommand | Implemented | `sponzey controller init/start` |
| controller data directory 초기화 | Implemented | key, admin token hash, SQLite store 생성 |
| Web Admin static serving | Implemented | `/admin` |
| OpenAPI JSON과 Swagger UI | Implemented | `/openapi.json`, `/swagger-ui` |
| HTTP controller URL 허용 | Implemented | test-only warning과 audit 필요 |
| Built-in HTTPS | Implemented | `--tls-cert`, `--tls-key` |
| Reverse proxy HTTPS | Implemented | `--external-url https://...` |
| Persistent agent session registry | Implemented | agent outbound WebSocket session |
| Immediate task dispatch | Implemented | active authenticated session으로 push |
| 세분화된 assignment ack/start/reject state | Implemented | task_ack, task_started, task_rejected protocol event와 store 상태 |
| Job cancel API와 task cancel protocol | Implemented | `/api/jobs/{job_id}/cancel`, `task_cancel`, canceled terminal state |
| Timeout/cancel terminal 구분 | Implemented | command timeout은 expired, operator cancel은 canceled |
| Multi-agent fanout 상태 집계 | Partial | target list와 assignment는 있으나 제품형 fanout gate 필요 |
| Controller key rotation | Planned | 보안 hardening 후속 |
| Backup/restore command | Implemented | JSON archive, metadata, checksum, dry-run restore, overwrite guard |
| Postgres store | Planned | repository contract 이후 결정 |

## Agent

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| Agent enrollment/init | Implemented | `sponzey agent init`, alias `enroll` |
| Persistent outbound WebSocket session | Implemented | heartbeat와 task dispatch 분리 |
| Reconnect 지속 시도 | Implemented | `--once`, `--max-reconnect-attempts`로 변경 가능 |
| Controller fingerprint pinning | Implemented | enrollment 이후 controller identity 확인 |
| Signed task envelope 검증 | Implemented | signature, expiry, replay, target 검증 |
| Command task 실행 | Implemented | output chunk와 result 전송 |
| Command cancel/timeout process boundary | Implemented | cancel/timeout 시 runner가 child process kill 후 status 보고 |
| Runbook primitive 실행 | Partial | canonical v1alpha1 schema, legacy fixture, idempotent package/service/file.copy, safe port/process/facts/metrics primitive, common step result model |
| Drift check assignment 실행 | Implemented | signed drift job dispatch |
| Facts 수집 | Implemented | static inventory payload, memory/disk usage 제외, disk/partition/mount/network inventory 포함 |
| Metrics 수집 | Implemented | usage telemetry snapshot |
| Operational log upload | Implemented | 기본 30초, disable/interval 조정 가능 |
| Cancellation protocol | Planned | Task 005 |
| Capability declaration | Planned | Task 010 이후 |
| Least privilege execution mode | Planned | capability/approval 이후 |

## Web Admin

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| Admin token 입력 | Implemented | protected API Bearer token |
| Agent list/detail | Implemented | online/offline/revoked 표시 |
| Enrollment token 생성 | Implemented | Web Admin과 API |
| Command job 생성 | Implemented | high-risk/broad target은 approval request 생성 |
| Job output polling viewer | Implemented | raw output은 product log와 분리 |
| Facts inventory view | Implemented | grouped rendering and disk/mount table |
| Metrics chart | Implemented | range selector, recent samples, manual refresh |
| Drift latest/history 표시 | Implemented | latest and paged history |
| Audit list | Implemented | 최근 audit 조회 |
| Approval queue | Implemented | pending approvals, approve/reject, expire due |
| Target preview | Planned | Task 006, Task 017 |
| Runbook catalog/validate UI | Partial | runbook job creation and result status, full catalog/validation later |
| Policy assignment API | Implemented | policy source 저장, direct agent assignment, schedule 저장/조회 |
| Policy assignment UI | Implemented | policy save/list, selected-agent assignment, drift schedule |

## CLI

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| `--help`, `--version` | Implemented | top-level CLI 지원 |
| `controller init/start` | Implemented | data dir 기반 |
| `agent init/start` | Implemented | enrolled agent 실행 |
| `enroll-token create/list/revoke` | Implemented | controller data dir 사용 |
| `run` | Implemented | controller command job 생성 |
| `facts`, `metrics` | Implemented | 조회 CLI |
| `logs` local file tail | Partial | remote logs.tail은 후속 |
| `drift check` | Implemented | local policy check |
| `apply` | Partial | validation-only |
| `retention cleanup` | Implemented | explicit cleanup |
| `login`/admin profile | Planned | Task 009 |
| `upgrade` | Partial | dry-run upgrade policy inspection only |

## Packaging

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| npm wrapper `@sponzey/fleet` | Implemented | Rust binary launcher |
| Platform npm packages | Implemented | darwin/linux arm64/x64 |
| GitHub Actions npm publish | Implemented | matching tag workflow |
| Standalone release tarballs | Implemented | release workflow artifact packaging and checksum verification script |
| Linux glibc baseline check | Implemented | Ubuntu 22.04 build and check script |
| Linux systemd service commands | Implemented | install/start/status/logs/uninstall with dry-run support |
| Windows package | Planned | not published |
| `.deb`, `.rpm`, Homebrew, Docker | Planned | Task 015 이후 |
| One-line installer | Planned | version pinning/dry-run 필요 |

## API and OpenAPI

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| OpenAPI 3.1 JSON | Implemented | `/openapi.json` |
| Swagger UI | Implemented | `/swagger-ui` |
| Enrollment token API | Implemented | create/list/revoke |
| Agent inventory API | Implemented | list/detail/labels/revoke-key |
| Facts/metrics/drift latest API | Implemented | agent/controller time fields 포함 |
| Facts/metrics/logs/drift paging API | Implemented | cursor paging |
| Job command/runbook/drift API | Implemented | high-risk confirmation |
| Job output API | Implemented | polling |
| Public/internal endpoint classification | Implemented | docs/api.md surface table |
| OpenAPI snapshot compatibility gate | Implemented | controller route contract test and Web Admin schema coverage |
| Generated SDK | Deferred | Web Admin dependency-free client is covered; TS/Rust SDK package deferred |

## Security and Audit

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| Enrollment token hash storage | Implemented | raw token 1회 노출 |
| Admin token hash storage | Implemented | bootstrap token |
| Agent/controller key pairs | Implemented | Ed25519 identity |
| Controller fingerprint pinning | Implemented | agent enrollment 이후 |
| Signed task envelope | Implemented | command/runbook/drift |
| Replay nonce guard | Partial | persistent store hardening은 후속 |
| High-risk confirmation | Compatibility | `confirmed_high_risk`는 approval 대체 아님 |
| Approval request workflow | Partial | approve/reject/expire API 구현, RBAC 세분화 후속 |
| RBAC/admin identity | Partial | bootstrap admin actor, owner/admin/operator/viewer role matrix, route permission checks |
| Audit events | Partial | 주요 이벤트 중심, schema hardening 후속 |
| Secret redaction | Partial | logs/output 경계 계속 보강 필요 |
