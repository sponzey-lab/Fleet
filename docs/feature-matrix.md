# Sponzey Fleet Feature Matrix

작성일: 2026-07-07
기준 구현: v0.0.18 현재 워킹트리, `.tasks/phase005` 완료 작업 및 루트 `.tasks/plan.md` Post-MVP 계획 기준

상태 표기:

- `Implemented`: 현재 동작하고 문서/테스트가 대체로 존재한다.
- `Partial`: 기본 흐름은 있으나 제품화에 필요한 상태 모델, UX, 테스트, 문서가 더 필요하다.
- `Planned`: 계획은 있으나 아직 구현되지 않았다.
- `Policy decision required`: 기술 구현 전에 제품 정책 결정이 필요하다.

## Post-MVP Execution Map

Planned 또는 Partial 항목은 다음 phase 번호를 기준으로 실행한다. 이 표에 없는
새 Post-MVP 기능은 루트 `.tasks/plan.md`에 phase를 먼저 추가한 뒤 task로
분해한다.

| 항목 | 현재 상태 | 다음 실행 phase | 선행 조건 |
| --- | --- | --- | --- |
| Template primitive / rendered artifact | Partial | Phase 1/4/5 | Plain-variable `file.template` parser/render/runner path, explicit SecretProvider-compatible resolver injection, TaskResult artifact metadata reporting, optional body persistence into local `ArtifactStore`, SQLite metadata storage, checksum drift contract, local `ArtifactStore` body contract, Controller artifact retrieval API, Web Admin artifact metadata/retrieval surface, typed `SecretRef`, application `SecretProvider` trait, disabled/static fake provider redaction tests, typed startup `SecretProviderSettings`, controller bootstrap provider factory, and agent runbook resolver handoff exist. Secret-backed template artifact body bytes are omitted by default. External adapters remain. |
| Artifact metadata/store contract | Partial | Phase 4 | SQLite metadata table, controller metadata write path, filesystem-neutral `ArtifactStore` application contract, local filesystem implementation with checksum verification/path traversal rejection/delete contract, task result artifact body persistence, Controller artifact retrieval API, and Web Admin artifact metadata/retrieval surface exist. S3-compatible adapter remains. |
| Policy-based remediation lifecycle | Partial | Phase 2 | Request state machine, SQLite metadata persistence, persisted approval request, approved signed runbook job creation, Controller task-event lifecycle reconciliation, and feature-gated Postgres repository/runtime adapter dispatch exist. Verified signed drifted reports atomically create one idempotent Proposed request with origin/audits. A successful remediation result atomically creates one correlated signed verification drift Job and queues it after commit when disconnected. Before listener readiness, a bounded startup scan reconciles correlation-free pending verification records and safely audits unverifiable legacy rows without dispatch. Successful verification plus fresh compliant evidence after remediation execution atomically resolves the remediation and only its origin drift, regardless of report/result delivery order. Legacy manual running/result/verify API and CLI commands are deprecated and return `409`; Web Admin displays persisted lifecycle state without manual transition controls. Approval-bypassing execution remains out of scope |
| Postgres store | Partial | Phase 3 | `DatabaseSettings`, typed Postgres URL/SSL mode/connect timeout/pool parsing, blocking Postgres client pool boundary, minimum native TLS adapter for `sslmode=prefer/require`, SQLite shared repository contract harness, feature-gated Postgres migration skeleton, repository slices through RemediationRequest, `ControllerStore`/`ControllerStoreRef` boundary, feature-gated Controller Postgres open/migration, direct server runtime adapter dispatch, typed job+assignment transaction boundary, and queued-only dispatch claim/release contract exist. A 2026-08-31 local PostgreSQL 18.3 disposable runtime run passed all 13 ignored repository tests, each in a fresh database. Custom CA/client certificate rotation, scheduled drift/retention lease, and HA claim semantics remain follow-up tasks. |
| S3-compatible artifact store | Planned | Phase 4/5 | Decision recorded; adapter implementation deferred until typed bootstrap `ArtifactStoreSettings`, external secret reference credential handling, feature-gated contract tests, and redaction tests are in place |
| mTLS / certificate and key rotation | Partial | Phase 5 | `ControllerTrustSettings`, `TlsServerIdentitySettings`, `ControllerSigningIdentitySettings`, and `AgentClientCertificateTrust` separate TLS server identity, controller signing identity, and future agent client cert trust at startup. `--agent-client-ca-cert`/`agent_client_ca_cert_path` is parsed as explicit future mTLS trust material but rejected before serving requests until listener enforcement exists. Agent certificate lifecycle domain state machine, snapshot/restore boundary, application repository/use-case contract, SQLite/Postgres-shaped public metadata persistence foundation, public-only lifecycle update/ack protocol schema, controller ack observation/audit, internal controller update dispatch helper, admin protected status and issuance request API/CLI surfaces, and agent explicit rejection ack exist; issue/renew/activate/revoke public controller surfaces, agent-side certificate application, listener enforcement, revocation propagation, and runtime trust checks remain. Controller signing key rotation domain state machine, dual-trust decision policy, rotation-state persistence contract, application operation/audit boundary, signing material validation boundary, filesystem staging/swap boundary, bootstrap runtime guard, explicit signer selection context, agent-side controller signing trust bundle verification, trust-bundle update/ack protocol and session foundation, agent trust sidecar restart survival, read-only rotation status API/CLI, mutation API/CLI, restart-plan API/CLI, audited external restart-action API/CLI, admin-triggered trust-bundle rollout API/CLI, bounded retry coordinator API/CLI, already-current ack skip, staged rollout domain state machine/persistence/worker, and Web Admin staged rollout surface exist. In-process hot reload/self-restart is not a current product path without a future ADR and reload state machine |
| Secret provider / Vault boundary | Partial | Phase 5 | Typed `SecretRef`, application `SecretProvider` trait, disabled/static fake provider, explicit runner resolver injection, typed startup `SecretProviderSettings`, controller bootstrap provider factory, agent runbook resolver handoff, and redaction tests exist. Product provider source configuration, rotation/lease lifecycle, and Vault/OpenBao adapter remain |
| OIDC and project/team RBAC | Planned | Phase 6 | Route permission matrix와 audit actor contract |
| Git runbook/policy catalog sync | Partial | Current catalog plan | Public HTTPS source register, bounded async sync, durable source/revision/document metadata, explicit ready-revision activation, immutable Runbook/Policy provenance, protected API/CLI, and Runbooks/Policies Admin explorer exist. Private Git credentials, project scope, and pinned release-runner performance threshold remain separate work. |
| Slack/Teams notification and telemetry export | Planned | Phase 8 | Redacted summary payload contract |
| One-line installer and package expansion | Planned | Phase 9 | Signature verification and version pinning |
| Agent staged update policy | Planned | Phase 9 | Signed update artifact and rollback state machine |
| HA controller coordination | Planned | Phase 10 | Postgres transaction/lease semantics |
| Compliance audit hash chain and signed export | Planned | Phase 11 | Phase 5 signing/key policy |
| Windows/macOS agent support | Planned | Phase 12 | Platform capability adapter boundary |
| Ansible bridge import subset | Planned | Phase 13 | Fleet runbook schema validation |
| Plugin/external adapter boundary | Planned | Phase 14 | Signed manifest and disabled-by-default policy |
| 100-agent / 1,000-output scale gates | Planned | Phase 15 | P0/P1 production paths complete |

## Controller

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| 단일 `fleet` 바이너리의 controller subcommand | Implemented | `fleet controller init/start` |
| controller data directory 초기화 | Implemented | key, admin token hash, SQLite store 생성 |
| Web Admin static serving | Implemented | `/admin` |
| OpenAPI JSON과 Swagger UI | Implemented | `/openapi.json`, `/swagger-ui` |
| HTTP controller URL 허용 | Implemented | test-only warning과 audit 필요 |
| Built-in HTTPS | Implemented | `--tls-cert`, `--tls-key`; typed trust settings keep TLS server material separate from controller signing keys |
| Reverse proxy HTTPS | Implemented | `--external-url https://...` |
| Persistent agent session registry | Implemented | agent outbound WebSocket session |
| Immediate task dispatch | Implemented | active authenticated session으로 push |
| 세분화된 assignment ack/start/reject state | Implemented | task_ack, task_started, task_rejected protocol event와 store 상태 |
| Job cancel API와 task cancel protocol | Implemented | `/api/jobs/{job_id}/cancel`, `task_cancel`, canceled terminal state |
| Timeout/cancel terminal 구분 | Implemented | command timeout은 expired, operator cancel은 canceled |
| Multi-agent fanout 상태 집계 | Implemented | selector target snapshot, assignment, concurrency, maxFailures, partial_success 계산 |
| Controller key rotation | Partial | Phase 5: domain signing key rotation state machine, dual-trust window decision policy, SQLite/Postgres-shaped persistence contract, application request/validate/activate/retire/fail operation use cases, key material sign/verify validation guard, filesystem staging/swap rollback boundary, bootstrap runtime guard, security audit boundary, explicit signer selection context, agent-side controller signing trust bundle verification, trust-bundle update/ack protocol and session foundation, agent trust sidecar restart survival, read-only status API/CLI, mutation API/CLI, restart-plan API/CLI, audited external restart-action API/CLI, admin-triggered trust-bundle rollout API/CLI, bounded retry coordinator API/CLI, already-current ack skip, staged rollout domain state machine/persistence/worker, and Web Admin staged rollout surface exist. In-process hot reload/self-restart is not a current product path without a future ADR and reload state machine |
| Backup/restore command | Implemented | JSON archive, metadata, checksum, dry-run restore, overwrite guard |
| Scheduled drift worker | Implemented | due schedule을 controller-signed drift-check job으로 생성하고 missed/skip audit 기록. Phase 10 전까지 HA-safe claim/lease는 없음 |
| Background retention worker | Implemented | controller-managed worker와 explicit cleanup command가 같은 application use case 사용. Phase 10 전까지 HA-safe retention lease는 없음 |
| Postgres store | Partial | Phase 3: typed database settings with startup-only Postgres URL/SSL mode/connect timeout/pool parsing, blocking Postgres client pool boundary, minimum native TLS adapter for `sslmode=prefer/require`, shared SQLite repository contract harness, feature-gated migration skeleton, repository slices through RemediationRequest, `ControllerStore`/`ControllerStoreRef` boundary, feature-gated Controller Postgres open/migration, direct Controller server runtime adapter dispatch, typed job+assignment transaction boundary, and queued-only dispatch claim/release contract exist. Custom CA/client certificate rotation, scheduled drift/retention lease, and HA claim semantics remain |

## Agent

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| Agent enrollment/init | Implemented | `fleet agent init`, alias `enroll` |
| Persistent outbound WebSocket session | Implemented | heartbeat와 task dispatch 분리 |
| Reconnect 지속 시도 | Implemented | `--once`, `--max-reconnect-attempts`로 변경 가능 |
| Controller fingerprint pinning | Implemented | enrollment 이후 controller identity 확인 |
| Signed task envelope 검증 | Implemented | signature, expiry, replay, target 검증 |
| Command task 실행 | Implemented | output chunk와 result 전송 |
| Command cancel/timeout process boundary | Implemented | cancel/timeout 시 runner가 child process kill 후 status 보고 |
| Runbook primitive 실행 | Partial | package/service/file.copy와 plain-variable/secret-ref file.template 실행 경로 존재. Secret-backed rendering uses explicit agent resolver handoff and omits artifact body bytes by default. `fleet apply` remains validation-only and does not resolve secrets. file.template은 TaskResult artifact metadata와 optional non-secret rendered body를 controller에 보고한다. Local `ArtifactStore` body persistence, Controller retrieval API, Web Admin artifact surface, typed `SecretRef`, and application `SecretProvider` contract exist |
| Drift check assignment 실행 | Implemented | signed drift job dispatch. File SHA-256 policy checks require 64-char lowercase hex and can use rendered template artifact checksum as expected value |
| Facts 수집 | Implemented | static inventory payload, memory/disk usage 제외, disk/partition/mount/network inventory 포함 |
| Metrics 수집 | Implemented | usage telemetry snapshot |
| Operational log upload | Implemented | 기본 30초, disable/interval 조정 가능 |
| Cancellation protocol | Implemented | `task_cancel`, process kill boundary, `canceled` terminal result |
| Capability declaration | Implemented | protocol snapshot, SQLite latest snapshot, Agent API/Web Admin summary, dispatch gate source of truth |
| Least privilege execution mode | Partial | unsupported/stale stored capability는 dispatch 전 rejected 처리. Phase 12에서 platform capability adapter, Phase 15에서 scale smoke |

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
| Target preview | Implemented | selector preview endpoint와 Web Admin command/runbook job 생성 전 preview, warning, selected/disabled/offline count 표시 |
| Runbook catalog/validate UI | Partial | Runbook job creation and result status exist. Runbooks/Policies share a public catalog explorer for source, revision, and body-free document metadata with explicit register/sync/activate actions; selecting an activated catalog document for job creation remains a later UI flow. |
| Policy assignment API | Implemented | policy source 저장, direct agent assignment, schedule 저장/조회. Verified signed drifted evidence가 Proposed remediation request를 원자적으로 생성하며, TaskStarted/TaskResult가 persisted remediation lifecycle을 반영하고 success가 correlated signed verification Job을 생성한다. successful verification과 fresh-compliant evidence는 remediation과 origin drift를 원자적으로 resolve하며 startup recovery도 구현됐다. legacy manual lifecycle API는 `409` deprecated contract다 |
| Policy assignment UI | Implemented | policy save/list, selected-agent assignment, drift schedule와 persisted proposal/approval/running/pending verify/resolved lifecycle queue/detail을 표시한다. manual running/result/verify control은 제공하지 않는다 |

## CLI

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| `--help`, `--version` | Implemented | top-level CLI 지원 |
| `controller init/start` | Implemented | data dir 기반 |
| `agent init/start` | Implemented | enrolled agent 실행 |
| `enroll-token create/list/revoke` | Implemented | controller data dir 사용 |
| `run` | Implemented | controller command job 생성 |
| `facts`, `metrics` | Implemented | 조회 CLI |
| `logs` local file tail | Partial | local file tail 구현. Remote managed log source expansion은 Phase 8 telemetry/export boundary 이후 별도 task |
| `drift check` | Implemented | local policy check |
| `apply` | Partial | validation-only. Catalog activation is a separate explicit revision-pointer change; execution continues through the signed job path only. |
| `retention cleanup` | Implemented | explicit cleanup |
| `audit export` | Implemented | category filter, cursor paging, JSONL renderer |
| `login`/admin profile | Implemented | `.fleet/cli-profile.json`, owner-only permission check, remote operator commands |
| `upgrade` | Partial | dry-run upgrade policy inspection only. Phase 9에서 signed staged update policy |

## Packaging

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| npm wrapper `@sponzey/fleet` | Implemented | Rust binary launcher |
| Platform npm packages | Implemented | darwin/linux arm64/x64 |
| GitHub Actions npm publish | Implemented | matching tag workflow |
| Standalone release tarballs | Implemented | release workflow artifact packaging and checksum verification script |
| Linux glibc baseline check | Implemented | Ubuntu 22.04 build and check script |
| Linux systemd service commands | Implemented | install/start/status/logs/uninstall with dry-run support |
| Release signature verification | Partial | `SHA256SUMS.sig` sign/verify scripts와 gate smoke 지원. Phase 9에서 release public key publication/rotation 기준 확정 |
| Windows package | Planned | Phase 12 agent/service support 후 Phase 9 packaging track에서 publish gate 추가 |
| `.deb`, `.rpm`, Homebrew, Docker | Planned | Phase 9: package decision record, smoke command, signature/version pinning |
| One-line installer | Planned | Phase 9: dry-run, version pinning, checksum/signature verification |

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
| Audit export API | Implemented | `/api/audit/export`, category filter, cursor paging, redacted values |
| Public/internal endpoint classification | Implemented | docs/api.md surface table |
| OpenAPI snapshot compatibility gate | Implemented | controller route contract test and Web Admin schema coverage |
| Generated SDK | Deferred | Web Admin dependency-free client is covered. A public SDK needs project RBAC and catalog API stabilization in a separate phase. |

## Security and Audit

| 기능 | 상태 | 현재 기준 |
| --- | --- | --- |
| Enrollment token hash storage | Implemented | raw token 1회 노출 |
| Admin token hash storage | Implemented | bootstrap token |
| Agent/controller key pairs | Implemented | Ed25519 identity |
| Controller fingerprint pinning | Implemented | agent enrollment 이후 |
| Signed task envelope | Implemented | command/runbook/drift |
| Replay nonce guard | Implemented | agent local file-backed nonce store, restart 이후 replay 거부, corruption fail-closed |
| High-risk confirmation | Compatibility | `confirmed_high_risk`는 approval 대체 아님 |
| Approval request workflow | Implemented | pending approval 생성, approve/reject/expire API, Web Admin queue |
| RBAC/admin identity | Partial | bootstrap admin actor, owner/admin/operator/viewer role matrix, route permission checks. Phase 6에서 OIDC/project scope |
| Audit events | Partial | 주요 이벤트와 category registry/export는 구현. Phase 11에서 tamper-evident chain/signed manifest |
| Audit export | Implemented | API/CLI category filter, cursor paging, SecretRef marker, update/delete API 없음 |
| Secret redaction | Partial | logs/output 경계 계속 보강 필요 |
