# Task 002: Migration/Repository Contract 최소 규칙과 Schema 변경 준비

상태: Completed
우선순위: P0
연결 계획: `.tasks/plan.md` Phase 011 선적용
의존성: Task 001 권장
결과물: job/assignment schema 변경 전 repository와 migration 기준선

## 목표

job/assignment lifecycle을 바꾸기 전에 storage 변경 규칙을 먼저 잡는다. 이후 task에서 job, assignment, approval, policy 등의 schema가 늘어날 예정이므로, schema 변경이 ad hoc으로 흩어지지 않도록 최소 migration/repository contract를 만든다.

이 task는 전체 backup/restore 구현이 아니라, Phase 005 작업을 안전하게 시작하기 위한 storage 기반 작업이다.

## 기능 묶음

1. SQLite schema/migration 기준선 정리
2. repository contract와 store test fixture 정리
3. job/assignment schema 변경 준비

## 구현 체크리스트

Schema 기준선:

- [x] 현재 SQLite schema 생성 경로를 확인한다.
- [x] schema version을 어디에 저장하는지 확인한다.
- [x] migration이 코드 기반인지 SQL file 기반인지 현재 구조를 문서화한다.
- [x] empty DB 초기화와 기존 DB migration 경로를 구분한다.
- [x] data dir 초기화 실패 시 error message가 명확한지 확인한다.
- [x] 기존 사용자의 DB를 깨지 않도록 backward migration fixture 필요성을 정리한다.

Repository contract:

- [x] agent repository contract를 확인한다.
- [x] job repository contract를 확인한다.
- [x] job output repository contract를 확인한다.
- [x] facts/metrics/drift repository contract를 확인한다.
- [x] audit repository contract를 확인한다.
- [x] 새 assignment repository가 들어갈 경계를 정한다.
- [x] repository trait이 domain/application 계층에 맞는지 확인한다.
- [x] infrastructure error가 domain error로 새지 않는지 확인한다.

Job/Assignment 준비:

- [x] 현재 job table과 job output table 구조를 조사한다.
- [x] assignment table이 필요한 필드를 설계한다.
- [x] assignment state transition에 필요한 timestamp 필드를 정리한다.
- [x] target snapshot 저장 방식을 설계한다.
- [x] output chunk와 final result 저장 경계를 설계한다.
- [x] migration 순서를 정한다.

## 테스트

- [x] empty DB initialization test를 추가하거나 확인한다.
- [x] current schema fixture에서 migration되는 테스트를 추가한다.
- [x] repository contract test fixture를 만든다.
- [x] migration이 기존 job 실행 smoke를 깨지 않는지 확인한다.
- [x] audit table이 retention/migration 작업으로 손상되지 않는지 기본 검증한다.

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
git diff --check
```

schema나 repository code가 변경되면 다음도 실행한다.

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

## 문서 업데이트

- [x] docs/storage.md 또는 기존 storage 관련 문서에 migration 기준을 정리한다.
- [x] `.tasks/plan.md`의 storage 관련 계획과 충돌하지 않는지 확인한다.
- [x] data dir 초기화/삭제/복구 설명과 migration 설명을 혼동하지 않게 정리한다.

## 완료 기준

- [x] schema 변경 전 migration 기준선이 명확하다.
- [x] repository contract test를 추가할 위치가 명확하다.
- [x] assignment table 추가 전 필요한 필드와 migration 순서가 정리되어 있다.
- [x] Phase 005 job/assignment 작업이 ad hoc schema 변경 없이 시작될 수 있다.

## 진행 결과

- `crates/fleet-store`에 `schema_migrations` 테이블과 `CURRENT_SCHEMA_VERSION` 기록 경로를 추가했다.
- empty DB 초기화와 기존 job schema migration fixture를 테스트로 고정했다.
- retention cleanup이 `audit_events`를 삭제하지 않는 기본 검증을 추가했다.
- storage/migration/repository contract 기준을 `docs/storage.md`에 정리했다.

## 검증 결과

- [x] `cargo fmt --check`
- [x] `cargo test -p fleet-store`
- [x] `cargo test --workspace`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `git diff --check`
