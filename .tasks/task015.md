# Task 015: Packaging, Service, Upgrade

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 012
의존성: Task 001, Task 014 권장
결과물: npm 외 운영 설치와 service/upgrade 경로

## 목표

npm install만으로는 모든 운영 환경을 감당하기 어렵다. standalone artifact, service install, upgrade/recovery 경로를 제품 기준으로 정리한다.

## 기능 묶음

1. service install/status/uninstall
2. standalone artifact와 integrity 검증
3. upgrade channel과 recovery 정책

## 구현 체크리스트

Service:

- [x] Linux systemd unit template을 정리한다.
- [x] controller service install command를 설계한다.
- [x] agent service install command를 설계한다.
- [x] service uninstall command를 설계한다.
- [x] service status command를 설계한다.
- [x] service log command를 설계한다.
- [x] service env를 runtime patch 방식으로 쓰지 않게 한다.
- [x] service는 명시 args/config object 중심으로 구성한다.

Artifact:

- [x] standalone tar.gz artifact를 공식화한다.
- [x] artifact naming 규칙을 정한다.
- [x] SHA256SUMS 생성을 확인한다.
- [x] signature 생성/검증 경로를 검토한다.
- [x] npm platform package와 standalone artifact의 차이를 문서화한다.
- [x] Windows 지원 여부를 명확히 결정하거나 미지원으로 표기한다.

Upgrade:

- [x] `sponzey upgrade` scope를 정한다.
- [x] stable/beta channel 정책을 정한다.
- [x] upgrade 전 backup 권장 또는 자동 확인을 설계한다.
- [x] upgrade 실패 시 rollback/recovery 문서를 만든다.
- [x] one-line installer dry-run과 version pinning을 설계한다.
- [x] Homebrew/Docker/.deb/.rpm은 별도 task로 나눌지 결정한다.

## 테스트

- [x] npm global install smoke
- [x] standalone archive smoke
- [x] checksum verification test
- [x] service install smoke
- [x] service status smoke
- [x] service uninstall smoke
- [x] upgrade dry-run test if implemented
- [x] release readiness gate update

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
npm test --workspace @sponzey/fleet
git diff --check
```

release script 변경 시:

```bash
./scripts/release_readiness_gate.sh
```

## 문서 업데이트

- [x] README에 npm/standalone/service 설치 차이를 정리한다.
- [x] docs/service-install.md를 최신화한다.
- [x] npm/fleet/README.md에 PATH troubleshooting을 보강한다.
- [x] release notes에 artifact와 upgrade 정책을 기록한다.

## 완료 기준

- [x] npm 외 공식 설치 경로가 최소 하나 검증된다.
- [x] service install/uninstall/status가 smoke test를 가진다.
- [x] artifact integrity 검증 경로가 있다.
- [x] upgrade 실패 시 recovery 설명이 있다.

## 구현 메모

- `sponzey controller status-service`, `sponzey agent status-service`를 추가했다.
- `sponzey controller logs-service`, `sponzey agent logs-service`를 추가했다.
- service status/log 조회는 Linux/systemd만 요구하고 root 권한은 요구하지 않는다.
- install/start/uninstall은 기존처럼 Linux root guard를 유지한다.
- `sponzey upgrade --dry-run`을 추가했다. 자동 self-upgrade는 아직 구현하지 않고, 외부 package/artifact 교체 전 backup, integrity check, recovery 절차를 출력하는 planning command로 제한했다.
- standalone artifact 검증 스크립트 `scripts/verify_standalone_artifacts.sh`를 추가했다.
- release tarball naming은 `sponzey-<os>-<arch>.tar.gz`로 공식화했다.
- `SHA256SUMS` 검증은 구현했지만 release signature 생성/검증은 미구현으로 남기고 후속 hardening task로 분리한다.
- Windows production service package는 현재 미지원으로 문서화했다.
- `.deb`, `.rpm`, Homebrew, Docker, one-line installer는 별도 후속 packaging task로 둔다.
