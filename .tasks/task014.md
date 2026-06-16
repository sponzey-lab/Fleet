# Task 014: Backup/Restore

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 011
의존성: Task 002, Task 013 권장
결과물: controller data backup/restore 경로

## 목표

production beta 전에 controller data dir을 백업하고 복구할 수 있는 공식 경로를 만든다. SQLite 기반 소규모 운영에서도 데이터 손실 위험을 낮춘다.

## 기능 묶음

1. backup command
2. restore command와 dry-run
3. integrity check와 문서화

## 구현 체크리스트

Backup:

- [x] 현재 data dir 구조를 조사한다.
- [x] controller DB 파일 위치를 확인한다.
- [x] artifact/job output 저장 위치를 확인한다.
- [x] backup 대상과 제외 대상을 정한다.
- [x] `sponzey controller backup` command를 설계한다.
- [x] backup metadata를 포함한다.
- [x] version, created_at, source data dir, schema version을 포함한다.
- [x] running controller에서 backup할 때의 정책을 정한다.

Restore:

- [x] `sponzey controller restore` command를 설계한다.
- [x] restore dry-run을 지원한다.
- [x] existing data dir overwrite 정책을 정한다.
- [x] restore 전 compatibility check를 수행한다.
- [x] restore 실패 시 partial write를 방지한다.
- [x] restore 후 integrity check를 수행한다.

Integrity:

- [x] SQLite integrity check를 실행할지 결정한다.
- [x] checksum metadata를 포함할지 결정한다.
- [x] backup archive format을 정한다.
- [x] sensitive data 포함 경고를 문서화한다.

## 테스트

- [x] backup creates archive test
- [x] backup metadata test
- [x] restore dry-run test
- [x] backup/restore roundtrip test
- [x] restore refuses incompatible version test
- [x] restore refuses overwrite without explicit confirmation test
- [x] integrity check failure test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] README에 data dir 백업/복구 기본 흐름을 추가한다.
- [x] docs/storage.md에 backup/restore 상세를 추가한다.
- [x] 운영 중 backup 시 주의사항을 문서화한다.

## 완료 기준

- [x] backup/restore roundtrip이 테스트된다.
- [x] restore dry-run으로 위험을 줄일 수 있다.
- [x] 기존 data dir overwrite가 안전하게 보호된다.
- [x] 사용자가 data dir 삭제와 backup/restore의 차이를 이해할 수 있다.

## 구현 메모

- Backup format은 dependency를 늘리지 않는 JSON archive다.
- Archive metadata는 `format`, `format_version`, `package_version`, `created_at_ms`, `source_data_dir`, `schema_version`, `sqlite_integrity_check`를 포함한다.
- Archive file entry는 `controller/...` 상대 경로, size, SHA-256 checksum, hex-encoded content를 포함한다.
- Backup 대상은 `controller/` 하위 regular file이다. 현재 DB, controller public/private key가 포함된다.
- Agent local identity는 controller backup에 포함하지 않는다.
- SQLite `fleet.db-wal`, `fleet.db-shm` transient file이 있으면 backup을 거부한다. 운영 중 백업은 controller를 중지한 뒤 실행하는 것을 공식 경로로 문서화했다.
- Restore dry-run은 format/version/checksum/schema compatibility를 확인하고 파일을 쓰지 않는다.
- 실제 restore는 임시 directory에 먼저 풀고 SQLite integrity check를 통과한 뒤 controller directory를 교체한다.
- 기존 controller directory가 비어 있지 않으면 `--force` 없이 restore하지 않는다.
- Backup archive에는 controller private key와 token hash가 포함되므로 secret처럼 취급해야 한다.