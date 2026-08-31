# Sponzey Fleet Release Gate

작성일: 2026-07-07

이 문서는 release 전 반드시 확인할 검증 명령과 smoke check를 정리한다.

## Required Local Gate

일반 개발 머신에서 release 후보를 확인할 때 실행한다.

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test --workspace @sponzey/fleet
npm test --workspace web-admin
npm run typecheck --workspace web-admin
./scripts/npm_local_pack_smoke.sh
./scripts/npm_platform_local_install_smoke.sh
./scripts/npm_demo_smoke.sh
./scripts/smoke_mvp.sh
./scripts/smoke_immediate_dispatch.sh
./scripts/smoke_remote_tls_loopback.sh
./scripts/signature_verification_smoke.sh
./scripts/storage_decision_gate.sh
./scripts/hardening_audit.sh
git diff --check
```

## Focused Store Gate

Schema, repository, backup/restore compatibility를 변경한 task는 Required Local Gate에
앞서 아래 빠른 gate를 먼저 실행한다.

```bash
cargo test -p fleet-store migration_from_versioned_previous_fixture_adds_columns_without_losing_rows
cargo test -p fleet-store sqlite_store_implements_application_repository_contracts
cargo test -p fleet-cli controller_restore_refuses_incompatible_schema_version
```

이 gate는 test fixture path를 env var로 받지 않아야 하며, runtime DB URL 또는
process env를 중간에 변경하지 않아야 한다.

`dist/release/SHA256SUMS`가 존재하면 standalone artifact도 검증한다.

```bash
./scripts/verify_standalone_artifacts.sh dist/release
```

`dist/release/SHA256SUMS.sig`가 존재하면 release public key를 명시해 checksum
manifest signature도 검증한다.

```bash
./scripts/verify_release_signature.sh dist/release ./release-public-key.pem
```

Tagged releases and manual workflow runs are fail-closed: the release workflow
requires the `RELEASE_SIGNING_PRIVATE_KEY` repository secret and the matching
committed `docs/release-signing-public.pem`. It verifies the pair before
publishing `SHA256SUMS.sig` and the public key with tagged release assets. A
manual dry-run retains only `SHA256SUMS`, its detached signature, and the
public key as a short-lived verification artifact; it does not publish npm
packages or create a GitHub Release. This signing key is separate from npm
Trusted Publishing and is never written to an artifact.

`target/release/fleet`가 존재하면 Linux release binary는 glibc baseline도 확인한다.

```bash
./scripts/check_linux_glibc_baseline.sh target/release/fleet
```

## One Command Gate

가능하면 release 전 아래 script를 사용한다.

```bash
./scripts/release_readiness_gate.sh
```

이 script는 local pack, platform package, npm demo, MVP smoke, immediate
dispatch smoke, remote TLS loopback smoke, release signature fixture smoke,
artifact store decision/settings boundary check, optional standalone artifact
verification, optional real signature verification, hardening audit를 함께 실행한다.
실제 release signature 검증에 기본값이 아닌
public key가 필요하면 명시적으로 전달한다.

```bash
./scripts/release_readiness_gate.sh --release-public-key ./release-public-key.pem
```

## Manual Linux Gate

systemd, nginx runbook, reboot persistence처럼 destructive하거나 root 권한이
필요한 항목은 일반 local gate에 포함하지 않는다. Linux host에서 명시적으로
실행한다.

```bash
sudo ./scripts/release_readiness_gate.sh --include-manual
sudo reboot
sudo ./scripts/release_readiness_gate.sh --verify-manual-reboot
```

## Registry Gate

npm registry publish 이후 실제 global install 경로를 확인한다.

```bash
./scripts/release_readiness_gate.sh --include-registry
```

## Stale Documentation Scan

문서 갱신 task에서는 아래 scan을 반드시 확인한다.

```bash
rg -n --glob '!docs/release-gate.md' "dev-insecure-loopback|insecure remote HTTP|planned release package|fleet agent enroll|agent enroll --" README.md README.ko.md PROJECT.md docs npm
```

허용 가능한 결과:

- 과거 phase 설명 또는 compatibility note로 명확히 표시된 경우
- `agent enroll` alias를 의도적으로 compatibility note로 설명하는 경우

허용하지 않는 결과:

- 현재 getting started 경로에 존재하지 않는 옵션이 남은 경우
- HTTP 원격 사용 정책이 현재 구현과 반대로 설명된 경우
- 이미 publish되는 platform package를 planned로 설명하는 경우
- 현재 권장 UX인 `agent init` 대신 `fleet agent enroll`을 기본 예시로 사용하는 경우

## Current-State Review Scan

MVP 계획 또는 current-state 문서를 갱신하는 task에서는 아래 scan을 확인한다.
이 scan은 0건을 요구하는 gate가 아니라, 결과가 현재 feature matrix, release
notes, policy 문서의 Known Limits 또는 Planned/Partial 상태와 일치하는지
리뷰하는 manual gate다.

```bash
rg -n "follow-up|후속|not implemented|Planned|Current MVP Limits|Known Limits" docs README.md README.ko.md
```

허용 가능한 결과:

- `docs/feature-matrix.md`의 `Planned` 또는 `Partial` 상태가 `.tasks/plan.md`의 남은 phase와 일치하는 경우
- `docs/release-notes-mvp.md`의 Known Limits가 아직 구현되지 않은 multi-controller scheduled drift/retention coordination, official release key publication, capability hardening follow-up을 명시하는 경우
- README의 follow-up 설명이 즉시 사용 가능한 대체 command 또는 현재 제한을 함께 설명하는 경우

허용하지 않는 결과:

- 이미 구현된 기능을 current-state 문서에서 `not implemented`로 설명하는 경우
- `.tasks/plan.md`의 phase와 맞지 않는 과거 `Task NNN` 참조가 남은 경우
- policy, security, protocol 문서가 HTTP test-only, approval, audit, capability, replay 정책을 서로 다르게 설명하는 경우

## Completed MVP P0 Gate

`.tasks/phase005/plan.md`의 MVP P0 목표는 완료된 기준선으로 보관되어 있다. 현재
Required Local Gate에는 MVP에서 추가된 capability, replay, scheduled drift,
retention, schema fixture, CLI profile, Web Admin target preview, release
signature smoke가 포함되어야 한다. 해당 smoke가 빠지면 release 후보로 보지 않는다.

## Post-MVP Not Implemented Gate Map

루트 `.tasks/plan.md`의 Post-MVP phase는 아래 gate를 아직 요구한다. 이 표의
항목은 현재 release gate 실패 조건이 아니라, 해당 phase task가 구현될 때 추가해야
하는 gate다.

| Gate | Owning phase | Required validation shape |
| --- | --- | --- |
| Template/artifact gate | Phase 1 | template render/checksum/artifact metadata contract test and smoke |
| Remediation lifecycle gate | Phase 2 | remediation request/approval/job/result/audit integration test, verification report/result order convergence, fresh evidence/agent-policy-version-job mismatch rejection, origin-only resolution, and SQLite/Postgres atomic rollback contract |
| Postgres store gate | Phase 3 | shared repository contract for SQLite and Postgres |
| Artifact retention gate | Phase 4 | local `ArtifactStore` checksum and retention class smoke |
| mTLS/key rotation gate | Phase 5 | TLS loopback with certificate/key rotation state tests |
| OIDC/RBAC gate | Phase 6 | route permission matrix and project-scope integration test |
| Git catalog gate | Current catalog plan | fake Git sync, validation error, activation audit, durable metadata paging, and 1,000-document deterministic reuse smoke; pinned-runner timing threshold remains separate |
| Notification/export gate | Phase 8 | redacted webhook payload and exporter mapping tests |
| Installer/package/update gate | Phase 9 | one-line installer dry-run, package smoke, signed update artifact test |
| HA coordination gate | Phase 10 | two-controller lease/claim simulation |
| Compliance audit gate | Phase 11 | hash-chain tamper detection and signed manifest verification |
| Cross-platform gate | Phase 12 | Windows/macOS manual gates separated from default local gate |
| Ansible bridge gate | Phase 13 | subset conversion fixtures and no-execution import test |
| Plugin adapter gate | Phase 14 | signed manifest, disabled-by-default, timeout/output limit tests |
| Scale/load gate | Phase 15 | 100 synthetic agents and 1,000 output chunks with bounded Product Log |

## Release Gate 원칙

- release gate는 runtime env patch나 숨은 config file을 요구하지 않아야 한다.
- HTTP smoke는 test-only warning이 유지되는지 확인해야 한다.
- HTTPS smoke는 production path가 문서상 존재함을 확인해야 한다.
- Web Admin smoke는 API schema와 UI client가 어긋나지 않게 해야 한다.
- registry smoke는 사용자 설치 경로에서 `fleet --help`가 실행되는지 확인해야 한다.
