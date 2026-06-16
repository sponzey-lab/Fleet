# Sponzey Fleet Release Gate

작성일: 2026-06-15

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
./scripts/hardening_audit.sh
git diff --check
```

`dist/release/SHA256SUMS`가 존재하면 standalone artifact도 검증한다.

```bash
./scripts/verify_standalone_artifacts.sh dist/release
```

`target/release/sponzey`가 존재하면 Linux release binary는 glibc baseline도 확인한다.

```bash
./scripts/check_linux_glibc_baseline.sh target/release/sponzey
```

## One Command Gate

가능하면 release 전 아래 script를 사용한다.

```bash
./scripts/release_readiness_gate.sh
```

이 script는 local pack, platform package, npm demo, MVP smoke, immediate
dispatch smoke, remote TLS loopback smoke, optional standalone artifact
verification, hardening audit를 함께 실행한다.

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
rg -n --glob '!docs/release-gate.md' "dev-insecure-loopback|insecure remote HTTP|planned release package|sponzey agent enroll|agent enroll --" README.md README.ko.md PROJECT.md docs npm
```

허용 가능한 결과:

- 과거 phase 설명 또는 compatibility note로 명확히 표시된 경우
- `agent enroll` alias를 의도적으로 compatibility note로 설명하는 경우

허용하지 않는 결과:

- 현재 getting started 경로에 존재하지 않는 옵션이 남은 경우
- HTTP 원격 사용 정책이 현재 구현과 반대로 설명된 경우
- 이미 publish되는 platform package를 planned로 설명하는 경우
- 현재 권장 UX인 `agent init` 대신 `sponzey agent enroll`을 기본 예시로 사용하는 경우

## Release Gate 원칙

- release gate는 runtime env patch나 숨은 config file을 요구하지 않아야 한다.
- HTTP smoke는 test-only warning이 유지되는지 확인해야 한다.
- HTTPS smoke는 production path가 문서상 존재함을 확인해야 한다.
- Web Admin smoke는 API schema와 UI client가 어긋나지 않게 해야 한다.
- registry smoke는 사용자 설치 경로에서 `sponzey --help`가 실행되는지 확인해야 한다.
