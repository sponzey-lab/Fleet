# Sponzey Fleet Storage and Migration Guide

작성일: 2026-07-07

이 문서는 Controller store의 현재 기준선과 이후 schema 변경 규칙을 정리한다.
목표는 job/assignment lifecycle, approval, policy 같은 다음 단계 schema 변경을
ad hoc으로 추가하지 않도록 하는 것이다.

## 현재 Storage 범위

현재 production-ready 기본 store는 SQLite다.

- 기본 controller DB는 data directory 아래 `controller/fleet.db`에 둔다.
- `sponzey controller start --db sqlite://...`로 SQLite DB 경로를 bootstrap 시점에 명시할 수 있다.
- `postgres://...`와 `postgresql://...` URL은 typed `DatabaseSettings`에서
  Postgres backend로 분류된다. `fleet-controller --features postgres`에서는
  explicit typed `PostgresConnectionSettings`로 `PostgresStore`를 열고 migration을
  실행한 뒤 Controller runtime repository adapter 뒤로 전달한다. `postgres`
  feature가 꺼진 build는 Postgres backend를 bootstrap에서 거부한다. request
  handler, UI, runtime API에서 DB backend를 바꾸는 경로는 없다.
- Postgres connection settings는 startup-only 값이다. URL query의 `sslmode`는
  `disable`, `prefer`, `require`만 허용하고 기본값은 `disable`이다. URL query의
  `connect_timeout`은 초 단위 양의 정수만 허용하고 기본값은 10초다. URL query의
  `pool_max_connections`는 양의 정수만 허용하고 기본값은 4다. URL query의
  `pool_checkout_timeout`은 초 단위 양의 정수만 허용하고 기본값은 5초다. 현재
  `PostgresStore`는 blocking `postgres` client pool을 infrastructure 내부에서
  초기화하고 repository method는 pool checkout helper를 통해 client를 얻는다.
  Checkout failure는 URL, username, password, host를 포함하지 않는 high-level
  store error로 반환되어야 한다. `sslmode=disable`은 `NoTls`로 연결하고,
  `sslmode=require`는 native platform TLS adapter로 server TLS 연결을 시도한다.
  `sslmode=prefer`는 TLS 연결을 먼저 시도하고 실패하면 `NoTls` 연결을 한 번
  시도한다. 현재 범위는 server certificate 검증을 platform trust store에
  위임하는 최소 adapter이며 custom CA, client certificate/mTLS, certificate
  rotation은 Phase 5 작업이다.
- Postgres store는 feature-gated implementation slice다. `fleet-store --features
  postgres`는 explicit URL을 받는 `PostgresStore::connect`, schema migration entrypoint,
  ignored integration migration test, Agent/AdminToken/ControllerIdentity/AgentIdentity
  repository trait implementation, EnrollmentToken repository trait implementation, Audit
  writer/query/export repository trait implementation, ApprovalRequest repository trait
  implementation, basic Job/TaskAssignment persistence repository implementation, Command/Runbook/Drift
  job repository implementation, DispatchAssignmentRepository pending assignment query와
  dispatch/reject/running/expired state transition, JobOutput/Facts/Metrics/AgentLog
  repository implementation, Drift/Policy/AgentCapability repository implementation,
  JobQuery/ArtifactMetadata/Retention repository implementation, RemediationRequest repository
  implementation을 제공한다. `fleet-controller --features postgres`는 direct
  `ControllerStore`/`ControllerStoreRef` dispatch를 통해 이 repository slice를
  server runtime handler에서 사용한다.
- runtime 중 DB URL이나 data directory를 바꾸는 방식은 허용하지 않는다.

현재 store는 `crates/fleet-store`의 `SqliteStore`가 담당한다.

현재 기본 data directory 구조:

```text
<data-dir>/
  controller/
    fleet.db
    controller_public.key
    controller_private.key
  agent/
    agent.conf
    agent_private.key
```

Controller backup/restore는 `controller/` 하위 파일만 대상으로 한다. `agent/`
하위 파일은 각 agent host의 local identity이므로 controller backup에 포함하지
않는다. 현재 job output, rendered artifact metadata, remediation request metadata,
facts, metrics, drift, audit, enrollment token hash는 SQLite `controller/fleet.db` 안에 저장된다.
`file.template` runner 결과는 agent `task_result` artifact metadata로 controller에
보고된다. `task_result.artifacts[].content_bytes`가 있으면 Controller는 checksum과
size를 검증한 뒤 body를 local `ArtifactStore`에 저장한다. Metadata-only legacy payload는
계속 허용되지만 retrieval API에서는 body missing으로 처리될 수 있다. Phase 4에는
filesystem-neutral `ArtifactStore` application contract와 local filesystem implementation이
있다. Controller bootstrap은 typed immutable `ArtifactStoreSettings`를 한 번 만들고,
MVP default backend인 local filesystem root를 `<data-dir>/controller/artifacts`로
고정한 뒤 `LocalArtifactStore`를 dependency로 주입한다. 이 local implementation은
artifact id 기반 object key만 허용하고 checksum verification, corrupt/missing result,
idempotent delete를 제공한다. Controller artifact retrieval API는 job id와 artifact
id만 받아 metadata 확인 후 verified body를 반환한다. Web Admin은 artifact metadata와
verified body retrieval action을 제공한다. Runtime API, Web Admin, request payload, process env는 artifact store backend 또는 root를 변경하지 않는다.
S3-compatible object storage는 아직 없다.

## S3-Compatible Artifact Store Decision

Decision: defer S3-compatible adapter implementation in the current Phase 4
slice.

Reason:

- Local `ArtifactStore` contract, body persistence, retrieval API, and Web
  Admin retrieval surface are now verified.
- Remote object storage introduces credential source, TLS trust, endpoint
  policy, retry/backoff, and object-store consistency decisions that must not
  be hidden inside request handlers or mutable global state.
- Phase 5 still owns SecretProvider-backed rendering and broader secret
  lifecycle work. Until that boundary exists, adding a concrete S3 SDK adapter
  would either hard-code startup secrets into a low-level adapter or invent a
  storage-specific secret path.

Required adapter design:

- Application and Domain crates must not depend on an S3 SDK, HTTP client, filesystem path, or environment variable lookup.
- The S3-compatible implementation must live in infrastructure behind
  `fleet_application::ArtifactStore`.
- Local filesystem remains the default artifact store backend.
- The local default is represented by typed immutable `ArtifactStoreSettings`
  and resolves to `<data-dir>/controller/artifacts` during Controller bootstrap.
- Backend selection must happen during Controller bootstrap only. No runtime configuration mutation is allowed.
- Web Admin and REST APIs must never accept artifact root, bucket name,
  endpoint URL, access key, secret key, session token, object key, or signed URL
  as request payload.
- Credentials must be startup secrets or secret references. Raw credentials must not be stored in SQLite/Postgres tables, application logs, audit values, Web Admin state, or job output.
- Object keys must be derived from artifact id and retention class only.
  Destination paths and local filesystem paths must never become remote object
  keys.
- `put` succeeds only after checksum verification. `get` returns bytes only
  through the `ArtifactStore` contract. `verify` returns `Verified`, `Missing`,
  or `Corrupt`. `delete` must be idempotent for missing objects.
- Retry/backoff, if added, must be bounded and explicit. Request handlers must
  not wait on unbounded remote object retries.

Logging policy:

- Product Log may include backend kind, artifact id, retention class,
  checksum prefix, size, and high-level status.
- Field Debug Log may include redacted endpoint host, retry count, and latency.
- Logs must not include access key, secret key, session token, signed URL,
  bucket credentials, object body, local path, destination path, or full HTTP
  request/response body.

Test strategy:

- Keep local `ArtifactStore` contract tests as the default workspace tests.
- Add any S3-compatible adapter behind an explicit feature flag.
- Use a fake/in-memory compatible client for fast contract tests.
- Mark real S3/MinIO integration tests ignored or feature-gated and require
  explicit endpoint/credential input at test startup only.
- Add redaction tests for adapter errors before enabling production use.

Implementation trigger:

- Introduce typed immutable `ArtifactStoreSettings` for `local` and `s3`.
- Define secret reference/startup secret handling for remote credentials.
- Add a feature-gated infrastructure adapter that passes the same
  put/get/verify/delete contract as `LocalArtifactStore`.
- Wire Controller bootstrap selection through explicit settings object or
  dependency injection only.
- Update release readiness gates with local default tests and optional ignored
  remote integration instructions.

## Migration 방식

현재 migration은 Rust 코드 기반이다.

- `SCHEMA_SQL`은 새 DB 또는 없는 table을 생성한다.
- `SqliteStore::migrate()`는 `SCHEMA_SQL`을 실행한 뒤 필요한 additive migration을 수행한다.
- 현재 additive migration은 `ensure_column` 방식으로 기존 table에 빠진 column을 추가한다.
- 현재 schema version은 `schema_migrations` table의 `fleet_store` row에 기록한다.
- 현재 버전 상수는 `CURRENT_SCHEMA_VERSION`이다.

현재 기준:

```text
schema_migrations
  name = fleet_store
  version = CURRENT_SCHEMA_VERSION
```

현재 `CURRENT_SCHEMA_VERSION`은 15이다.

반복 실행 규칙:

- migration은 여러 번 실행해도 성공해야 한다.
- 이미 존재하는 table과 column은 유지해야 한다.
- 기존 row를 삭제하거나 재작성하지 않아야 한다.
- destructive migration은 별도 backup/restore 경로와 명시 operator 확인 없이는 허용하지 않는다.

## Empty DB 초기화와 Existing DB Migration 구분

Empty DB 초기화:

- data directory가 준비된 뒤 SQLite file을 연다.
- `SCHEMA_SQL`이 전체 table을 만든다.
- `schema_migrations`에 현재 version을 기록한다.

Existing DB migration:

- 기존 table과 row를 보존한다.
- 없는 table은 `CREATE TABLE IF NOT EXISTS`로 추가한다.
- 없는 column은 `ensure_column`으로 추가한다.
- 기존 column의 의미를 바꾸거나 삭제하지 않는다.
- backward fixture test로 기존 DB shape에서 현재 schema로 올라오는지 확인한다.

현재 migration fixture:

- `crates/fleet-store/fixtures/sqlite/schema_v8_jobs_only.sql`
- fixture는 `schema_migrations`의 `fleet_store` version을 명시해야 한다.
- fixture test는 env var로 path를 찾지 않고 Rust test에서 `include_str!`로 고정한다.
- fixture migration 성공은 column 존재 여부만이 아니라 `schema_migrations.version = CURRENT_SCHEMA_VERSION`과 legacy row 보존으로 확인한다.

Data directory 초기화 실패와 DB migration 실패는 구분한다.

- data directory가 없거나 권한이 없으면 controller init/start 경계에서 명확한 filesystem error를 보여준다.
- SQLite file은 열렸지만 schema 변경이 실패하면 store/migration error로 다룬다.
- 운영자는 data directory 삭제와 migration을 혼동하면 안 된다. 삭제는 reset이고, migration은 보존 upgrade다.

## Transaction Boundary

Application layer는 DB transaction object를 직접 받지 않는다. 다단계 저장 작업은
repository contract로 표현하고 SQLite/Postgres infrastructure adapter가 내부에서
transaction을 연다.

현재 명시된 boundary:

| Operation | Boundary | Failure behavior | HA note |
| --- | --- | --- | --- |
| typed job + assignments create | `save_*_job_with_assignments` repository method | job insert 후 assignment insert가 실패하면 job과 이전 assignment가 함께 rollback되어야 한다. | 단일 store transaction 기준이다. |
| dispatch claim | `claim_assignment_for_dispatch` | `queued -> dispatched` 전이만 성공한다. non-queued, missing, terminal assignment는 상태를 바꾸지 않고 false/no-op로 처리한다. | multi-controller duplicate dispatch 방지는 Phase 10 lease/advisory lock 작업이다. |
| dispatch claim release | `release_assignment_dispatch_claim` | send failure 후 `dispatched -> queued`만 허용한다. rejected/succeeded/failed/canceled/expired 같은 terminal 상태는 requeue하지 않는다. | active WebSocket send 성공을 accepted/started로 간주하지 않는다. |
| scheduled drift worker | `due_scheduled_drift_checks` + `record_scheduled_drift_check` | 현재는 단일 controller worker 기준으로 due list 조회 후 job 생성과 schedule update를 순차 수행한다. | HA-safe claim/lease는 아직 없으며 Phase 10 전까지 multi-controller 중복 실행 가능성을 지원 상태로 표현하지 않는다. |
| retention cleanup | `cleanup_retention` | dry-run은 count만 수행하고 삭제하지 않는다. 실제 cleanup은 artifact type별 cutoff를 사용한다. Audit table은 삭제 대상이 아니다. | HA-safe retention lease는 Phase 10 작업이다. |

검증 기준:

- `cargo test -p fleet-store transaction`
- `cargo test -p fleet-store sqlite_store_passes_typed_job_repository_contract_harness`
- `cargo test -p fleet-store sqlite_store_passes_dispatch_assignment_repository_contract_harness`
- `cargo test -p fleet-store --features postgres postgres_store_implements_typed_job_repository_traits`
- `cargo test -p fleet-store --features postgres postgres_store_implements_dispatch_assignment_repository_trait`

## Backup/Restore

공식 controller backup command:

```bash
sponzey controller backup \
  --data-dir .sponzey \
  --output ./sponzey-controller.backup.json
```

공식 restore command:

```bash
sponzey controller restore \
  --data-dir .sponzey-restored \
  --input ./sponzey-controller.backup.json
```

위험을 줄이기 위해 먼저 dry-run을 실행할 수 있다.

```bash
sponzey controller restore \
  --data-dir .sponzey-restored \
  --input ./sponzey-controller.backup.json \
  --dry-run
```

Archive format:

- JSON file
- `format = sponzey-controller-backup`
- `format_version = 1`
- `package_version`
- `created_at_ms`
- `source_data_dir`
- `schema_version`
- `sqlite_integrity_check`
- file list with relative path, size, SHA-256 checksum, hex-encoded content

Backup 대상:

- `controller/fleet.db`
- `controller/controller_public.key`
- `controller/controller_private.key`
- 앞으로 `controller/` 아래에 생기는 regular file

Backup 제외 대상:

- `agent/` local identity
- runtime socket, pid file, temporary file
- SQLite transient file `fleet.db-wal`, `fleet.db-shm`

운영 중 backup 정책:

- 권장 경로는 controller process를 중지한 뒤 backup하는 것이다.
- backup command는 SQLite `PRAGMA integrity_check`가 `ok`가 아니면 실패한다.
- `fleet.db-wal` 또는 `fleet.db-shm`이 남아 있으면 running/dirty SQLite 상태일 수 있어 backup을 거부한다.
- archive에는 controller private key와 token hash가 포함되므로 secret처럼 보관해야 한다.

Restore 안전장치:

- archive format/version을 확인한다.
- archive schema version이 현재 binary의 `CURRENT_SCHEMA_VERSION`보다 크면 거부한다.
- 각 file checksum과 size를 확인한다.
- archive path는 `controller/` 아래 상대 경로만 허용한다.
- dry-run은 파일을 쓰지 않고 복구 계획과 compatibility만 확인한다.
- 기존 `controller/` directory가 비어 있지 않으면 `--force` 없이 restore하지 않는다.
- 실제 restore는 임시 directory에 먼저 풀고 SQLite integrity check를 통과한 뒤 controller directory를 교체한다.
- restore 실패 시 target controller directory를 최대한 보존한다.

삭제와 backup/restore의 차이:

- data directory 삭제는 reset이다. controller identity, jobs, audit, telemetry, enrollment records가 사라진다.
- backup/restore는 보존 복구다. 같은 controller identity와 저장 데이터를 다시 사용할 수 있다.

## 현재 주요 Table

Core identity:

- `controller_identity`
- `admin_tokens`
- `agents`
- `agent_identities`
- `enrollment_tokens`

Job and execution:

- `jobs`
- `job_targets`
- `task_assignments`
- `job_output_chunks`
- `approval_decisions`
- `approval_requests`

Telemetry and drift:

- `facts_snapshots`
- `metrics_snapshots`
- `drift_reports`
- `agent_log_chunks`
- `agent_capability_snapshots`

Policy:

- `policies`
- `policy_assignments`
- `policy_drift_schedules`
- `remediation_requests`

Audit:

- `audit_events`

`audit_events`는 controller API/application 경계에서 append-only로 다룬다.
운영 export는 `/api/audit/export` 또는 `sponzey audit export`의 category
filter와 cursor paging을 사용한다. SQLite MVP 저장소는 물리적 WORM 보관소가
아니므로 장기 보존, 변조 방지, 규정 준수는 외부 백업/export 보관 정책으로
보완해야 한다.

Migration metadata:

- `schema_migrations`

## Repository Contract 기준

Application layer는 store 구현체가 아니라 repository trait을 통해 접근한다.

현재 주요 repository contract:

- `AgentRepository`
- `AgentInventoryRepository`
- `AgentIdentityRepository`
- `AdminTokenRepository`
- `ControllerIdentityRepository`
- `EnrollmentTokenRepository`
- `JobRepository`
- `CommandJobRepository`
- `RunbookJobRepository`
- `DriftCheckJobRepository`
- `TaskAssignmentRepository`
- `JobQueryRepository`
- `JobOutputRepository`
- `PolicyRepository`
- `FactsRepository`
- `MetricsRepository`
- `DriftRepository`
- `AgentCapabilityRepository`
- `AuditRepository`
- `ApprovalRepository`

규칙:

- Domain layer는 SQLite, SQL, filesystem을 알면 안 된다.
- Application layer는 repository trait과 typed domain/application record만 사용한다.
- Infrastructure error는 repository boundary 밖으로 raw SQL 문맥을 과도하게 새지 않도록 한다.
- Store test는 trait contract를 통해 최소 roundtrip을 검증한다.
- 새 table을 추가할 때는 해당 application repository boundary를 먼저 정의한다.

## Agent Capability Snapshot Schema

`agent_capability_snapshots`는 agent가 보고한 최신 capability snapshot을 agent별로
하나만 보관한다. 이 table은 active WebSocket session summary가 아니라 dispatch
gate와 Agent API 응답이 사용하는 persistent source of truth다.

현재 column:

- `agent_id`: `agents.id`를 참조하는 primary key
- `privilege_level`: `unprivileged`, `sudo_available`, `root`
- `package_manager`: `apt`, `dnf`, `yum`, `apk`, `brew`, 또는 null
- `service_manager`: `systemd`, `launchd`, `openrc`, 또는 null
- `capabilities_json`: agent가 보고한 capability name array JSON
- `reported_at`: agent가 snapshot을 만든 system time epoch seconds
- `updated_at`: controller store upsert time epoch seconds

규칙:

- runtime 중 capability override 설정을 추가하지 않는다.
- Domain layer는 SQL schema를 알지 않고 `AgentCapabilitySnapshot`만 다룬다.
- Dispatch gate는 stored snapshot이 있는 경우 24시간 기본 TTL과 `RuntimePrimitive` 요구사항을 평가한다.
- Unsupported snapshot은 assignment를 WebSocket write 전에 `rejected` terminal 상태로 저장한다.
- raw collector output, command output, secret-bearing payload를 capability table이나 Product Log에 저장하지 않는다.

## Job/Assignment Schema

job과 assignment 상태는 분리한다.

현재:

- `jobs.status`는 coarse job status다.
- `task_assignments`는 signed task envelope 저장, pending dispatch queue, assignment lifecycle state 저장 역할을 한다.
- `job_targets`는 job과 target agent의 현재 단순 status를 저장한다.
- `job_output_chunks`는 stdout/stderr chunk를 저장한다.
- `rendered_artifacts`는 rendered template의 metadata만 저장한다. Rendered body는
  `ArtifactStore` object로 분리 저장되며, template body와 secret value는 DB table에
  저장하지 않는다.
- `approval_requests.job_id`는 일반 job approval에서는 existing job id를, remediation
  approval에서는 approve 이후 생성될 reserved job id를 저장한다. 따라서
  `approval_requests.job_id`는 `jobs(id)` FK를 두지 않는다. Remediation approval이
  job row 생성보다 먼저 존재할 수 있어야 하며, job 생성과 assignment 저장은 approve
  use case에서 한 번에 처리한다.

`task_assignments` lifecycle column:

- `status`: `queued`, `dispatched`, `accepted`, `started`, `succeeded`, `failed`, `rejected`, `canceled`, `expired`
- `dispatched_at`
- `accepted_at`
- `started_at`
- `completed_at`
- `last_error`

Post-MVP schema 변경 후보와 실행 phase:

- target snapshot source fields: Phase 2 remediation/result correlation에서 필요한 경우 추가한다.
- selector snapshot fields: Phase 2 remediation과 Phase 10 HA dispatch claim에서 필요한 경우 추가한다.
- final result payload: Phase 2 remediation result report와 Phase 11 compliance report에서 필요한 경우 추가한다.
- output chunk와 final result 분리: Phase 11 compliance report와 Phase 15 load gate 전에 repository contract로 고정한다.
- rendered artifact protocol link: 구현됨. runner 결과는 `task_result.artifacts`로 controller/store metadata에 연결하고, optional `content_bytes`가 있으면 local `ArtifactStore` body로 저장한다.
- artifact blob storage: Phase 4에서 local `ArtifactStore` contract, checksum verification, path traversal rejection, idempotent delete boundary, task result body persistence, Controller artifact retrieval API, Web Admin artifact metadata/retrieval surface가 추가됐다. S3-compatible adapter는 후속 task다.
- Postgres migration metadata: Phase 3에서 SQLite와 같은 repository contract로 추가한다.

권장 migration 순서:

1. 새 nullable column 또는 새 table을 추가한다.
2. 기존 row가 새 schema에서 읽히는지 fixture test를 추가한다.
3. application repository가 새 필드를 optional/default로 읽게 한다.
4. domain state machine test를 추가한다.
5. controller/agent protocol event를 연결한다.
6. 충분히 안정화된 뒤에만 NOT NULL 제약이나 stricter invariant를 검토한다.

## Output과 Result 저장 경계

`job_output_chunks`는 raw stdout/stderr chunk 저장소다.

- Product application log에 raw output을 남기지 않는다.
- duplicate `(job_id, agent_id, stream, chunk_index)`는 같은 body면 idempotent로 처리한다.
- 같은 key에 다른 body가 들어오면 protocol conflict로 본다.

Final result는 output chunk와 다르다.

- success/failure/canceled/expired/rejected는 assignment final result로 저장해야 한다.
- exit code, duration, changed/skipped, primitive result는 final result에 가깝다.
- output chunk 수신만으로 job success를 판단하지 않는다.
- 이미 terminal 상태인 assignment는 늦은 result로 덮어쓰지 않는다. operator cancel 이후 늦은 success가 도착해도 job/assignment는 `canceled`로 남아야 한다.

## Retention과 Audit

현재 retention cleanup path는 explicit CLI command와 controller-managed worker를
포함하며 다음 table을 대상으로 한다.

- `job_output_chunks`
- `facts_snapshots`
- `metrics_snapshots`
- `agent_log_chunks`

Audit retention은 별도 operator policy가 생기기 전까지 자동 삭제하지 않는다.

규칙:

- retention worker나 cleanup command는 `audit_events`를 기본 삭제 대상에 넣지 않는다.
- cleanup 실행 자체는 audit event로 남겨야 한다.
- logs, job output, metrics, facts는 retention 정책을 서로 구분한다.
- audit 장기 보관은 일반 retention worker가 아니라 명시적인 operator export/backup
  정책으로 처리한다.

## Test 기준

Storage 변경 시 최소 확인:

- empty DB initialization test
- migration repeatability test
- previous schema fixture migration test
- repository contract roundtrip test through application repository traits
- duplicate/constraint behavior test
- retention does not delete audit events test
- backup/restore rejects newer schema archive test
- `cargo test --workspace`
- `git diff --check`

빠른 store gate:

```bash
cargo test -p fleet-store migration_from_versioned_previous_fixture_adds_columns_without_losing_rows
cargo test -p fleet-store sqlite_store_implements_application_repository_contracts
cargo test -p fleet-store sqlite_store_passes_shared_repository_contract_harness
cargo test -p fleet-store migration_state_transitions_follow_phase3_gate
cargo test -p fleet-application artifact_store_contract_is_storage_backend_neutral
cargo test -p fleet-store local_artifact_store
cargo test -p fleet-store --features postgres postgres_store_exposes_explicit_connection_entrypoints
cargo test -p fleet-store --features postgres postgres_store_selects_tls_adapter_for_sslmode_require
cargo test -p fleet-store --features postgres postgres_tls_adapter_failure_is_redacted
cargo test -p fleet-store --features postgres postgres_store_checkout_failure_is_redacted
cargo test -p fleet-store --features postgres postgres_store_implements_bootstrap_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_enrollment_token_repository_trait
cargo test -p fleet-store --features postgres postgres_store_implements_audit_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_approval_repository_trait
cargo test -p fleet-store --features postgres postgres_store_implements_job_assignment_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_typed_job_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_dispatch_assignment_repository_trait
cargo test -p fleet-store --features postgres postgres_store_implements_output_telemetry_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_drift_policy_capability_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_query_artifact_retention_repository_traits
cargo test -p fleet-store --features postgres postgres_store_implements_remediation_request_repository_trait
cargo test -p fleet-core database_settings
cargo test -p fleet-controller postgres_backend
cargo test -p fleet-cli controller_restore_refuses_incompatible_schema_version
```

Postgres가 있는 개발 환경에서만 실행하는 ignored gate:

```bash
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_migration_records_current_schema_version -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_bootstrap_repositories_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_enrollment_token_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_audit_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_approval_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_job_assignment_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_typed_job_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_dispatch_assignment_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_output_telemetry_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_drift_policy_capability_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_query_artifact_retention_repository_roundtrip -- --ignored
SPONZEY_TEST_POSTGRES_URL=postgresql://... \
  cargo test -p fleet-store --features postgres postgres_remediation_request_repository_roundtrip -- --ignored
```

이 URL은 test-only 입력이다. production code는 process env를 읽지 않고 explicit
argument 또는 bootstrap `DatabaseSettings`로만 DB 설정을 받는다.

Fixture 갱신 절차:

1. `CURRENT_SCHEMA_VERSION`을 올리는 schema 변경 전에 새 previous-version fixture를 만든다.
2. fixture에는 기존 row와 `schema_migrations` row를 함께 넣는다.
3. 실패하는 fixture migration test를 먼저 작성한다.
4. migration 구현 후 legacy row 보존, 새 column/default, current schema version 기록을 검증한다.
5. backup/restore schema compatibility test가 newer archive를 계속 거부하는지 확인한다.
6. 문서의 fixture 파일명과 빠른 store gate 명령을 함께 갱신한다.

Schema/repository code가 바뀌면 추가 확인:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

## 아직 하지 않는 것

- Postgres implementation: Phase 3. `DatabaseSettings`, SQLite shared repository
  contract harness, feature-gated Postgres migration skeleton, and ignored integration
  migration test exist. Agent/AdminToken/ControllerIdentity/AgentIdentity bootstrap
  repository traits, EnrollmentToken repository trait, Audit repository trait, and ApprovalRequest
  repository trait exist behind the `postgres` feature. Basic Job/TaskAssignment persistence
  traits, command/runbook/drift typed job repository traits, and DispatchAssignmentRepository
  pending query/state transition also exist. JobOutput/Facts/Metrics/AgentLog,
  Drift/Policy/AgentCapability, JobQuery/ArtifactMetadata/Retention, and RemediationRequest
  repositories also exist. Controller runtime now has `ControllerStore` and `ControllerStoreRef`
  boundaries, feature-gated Postgres open/migration wiring, direct repository adapter dispatch,
  and queued-only dispatch claim/release contract in `fleet-controller --features postgres`.
  Remaining Postgres/HA work is broader lease/claim semantics for scheduled drift, retention,
  and multi-controller workers. Custom CA, mTLS, and certificate rotation are Phase 5 work.
- S3-compatible artifact storage: Phase 4 local `ArtifactStore` contract와 checksum/path traversal/delete tests 통과 후 별도 task.
- destructive migration: backup/restore, operator confirmation, fixture migration test가 먼저 필요하다.
- runtime DB configuration mutation: 계속 금지한다.
- UI에서 DB schema 또는 runtime config를 직접 수정하는 기능: 계속 금지한다.
- HA lease/leader election: Phase 10.
- tamper-evident audit hash chain and signed manifest: Phase 11.
