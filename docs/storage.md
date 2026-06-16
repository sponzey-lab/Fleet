# Sponzey Fleet Storage and Migration Guide

작성일: 2026-06-15

이 문서는 Controller store의 현재 기준선과 이후 schema 변경 규칙을 정리한다.
목표는 job/assignment lifecycle, approval, policy 같은 다음 단계 schema 변경을
ad hoc으로 추가하지 않도록 하는 것이다.

## 현재 Storage 범위

현재 production-ready 기본 store는 SQLite다.

- 기본 controller DB는 data directory 아래 `controller/fleet.db`에 둔다.
- `sponzey controller start --db sqlite://...`로 SQLite DB 경로를 bootstrap 시점에 명시할 수 있다.
- Postgres store는 아직 구현하지 않았다.
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
않는다. 현재 job output, facts, metrics, drift, audit, enrollment token hash는
SQLite `controller/fleet.db` 안에 저장된다. 별도 artifact directory는 아직 없다.

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

현재 `CURRENT_SCHEMA_VERSION`은 9다.

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

Data directory 초기화 실패와 DB migration 실패는 구분한다.

- data directory가 없거나 권한이 없으면 controller init/start 경계에서 명확한 filesystem error를 보여준다.
- SQLite file은 열렸지만 schema 변경이 실패하면 store/migration error로 다룬다.
- 운영자는 data directory 삭제와 migration을 혼동하면 안 된다. 삭제는 reset이고, migration은 보존 upgrade다.

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

Telemetry and drift:

- `facts_snapshots`
- `metrics_snapshots`
- `drift_reports`
- `agent_log_chunks`

Policy:

- `policies`
- `policy_assignments`
- `policy_drift_schedules`

Audit:

- `audit_events`

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
- `AuditRepository`
- `ApprovalRepository`

규칙:

- Domain layer는 SQLite, SQL, filesystem을 알면 안 된다.
- Application layer는 repository trait과 typed domain/application record만 사용한다.
- Infrastructure error는 repository boundary 밖으로 raw SQL 문맥을 과도하게 새지 않도록 한다.
- Store test는 trait contract를 통해 최소 roundtrip을 검증한다.
- 새 table을 추가할 때는 해당 application repository boundary를 먼저 정의한다.

## Job/Assignment Schema

job과 assignment 상태는 분리한다.

현재:

- `jobs.status`는 coarse job status다.
- `task_assignments`는 signed task envelope 저장, pending dispatch queue, assignment lifecycle state 저장 역할을 한다.
- `job_targets`는 job과 target agent의 현재 단순 status를 저장한다.
- `job_output_chunks`는 stdout/stderr chunk를 저장한다.

`task_assignments` lifecycle column:

- `status`: `queued`, `dispatched`, `accepted`, `started`, `succeeded`, `failed`, `rejected`, `canceled`, `expired`
- `dispatched_at`
- `accepted_at`
- `started_at`
- `completed_at`
- `last_error`

후속 schema 변경 후보:

- target snapshot source fields
- selector snapshot fields
- final result payload
- output chunk와 final result 분리

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

현재 explicit retention cleanup은 다음 table을 대상으로 한다.

- `job_output_chunks`
- `facts_snapshots`
- `metrics_snapshots`
- `agent_log_chunks`

Audit retention은 별도 operator policy가 생기기 전까지 자동 삭제하지 않는다.

규칙:

- retention worker나 cleanup command는 `audit_events`를 기본 삭제 대상에 넣지 않는다.
- cleanup 실행 자체는 audit event로 남겨야 한다.
- logs, job output, metrics, facts는 retention 정책을 서로 구분한다.

## Test 기준

Storage 변경 시 최소 확인:

- empty DB initialization test
- migration repeatability test
- previous schema fixture migration test
- repository contract roundtrip test
- duplicate/constraint behavior test
- retention does not delete audit events test
- `cargo test --workspace`
- `git diff --check`

Schema/repository code가 바뀌면 추가 확인:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

## 아직 하지 않는 것

- Postgres implementation
- automatic background retention worker
- destructive migration
- runtime DB configuration mutation
- UI에서 DB schema 또는 runtime config를 직접 수정하는 기능
