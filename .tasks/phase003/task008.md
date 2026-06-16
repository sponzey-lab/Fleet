# Task 008 - Facts/Metrics/Log Interval 분리

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

telemetry 전송 주기와 task dispatch 반응성을 분리한다.

persistent session에서는 heartbeat가 연결 주기가 아니라 liveness signal이다. facts, metrics, log를 매 heartbeat마다 함께 보내면 비용과 의미가 섞인다.

## 기능 범위

### 1. Facts interval 분리

- [x] `--facts-interval-seconds` 옵션을 추가한다.
- [x] 기본값은 낮은 빈도로 둔다. 후보: 300초.
- [x] facts는 정적 inventory이므로 매 heartbeat마다 보내지 않는다.

Facts 기준:

- OS, arch, hostname
- CPU logical count
- total memory, memory module count
- disk device inventory, mount layout
- root filesystem capacity
- network interface names

금지:

- current memory usage
- current disk usage
- CPU usage percent

### 2. Metrics interval 분리

- [x] `--metrics-interval-seconds` 옵션을 추가한다.
- [x] 기본값은 chart에 필요한 빈도로 둔다. 후보: 30초.
- [x] metrics는 CPU/memory/disk/process/service usage를 다룬다.

Metrics 기준:

- CPU usage percent
- memory used/available/used percent
- disk used/available/used percent
- process count
- failed service count

### 3. Log upload interval session 통합

- [x] 기존 `--log-upload-interval-seconds`와 `--disable-log-upload` 정책을 persistent session loop에 통합한다.
- [x] 기본 30초 operational log upload를 유지한다.
- [x] raw file tail/journald stream이 아니라 product-safe operational log chunk만 보낸다.

## 테스트와 검증

필수:

- [x] facts가 매 heartbeat마다 오지 않는 test
- [x] metrics가 설정 interval에 맞춰 전송되는 test
- [x] log upload disable/interval test 유지
- [x] persistent session 중 telemetry tick이 task dispatch를 막지 않는 test
- [x] `cargo test -p fleet-cli facts`
- [x] `cargo test -p fleet-cli metrics`
- [x] `cargo test -p fleet-cli agent_session`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

CLI 검증:

- [x] `sponzey agent start --help`에 interval 의미가 명확히 표시된다.
- [x] 기존 `--heartbeat-interval-seconds` 설명은 liveness tick으로 변경된다.

## 완료 기준

- [x] task push는 telemetry 주기와 무관하다.
- [x] facts는 낮은 빈도 정적 inventory로 전송된다.
- [x] metrics는 chart 용도에 맞는 빈도로 전송된다.
- [x] operational log upload는 disable/interval 변경이 가능하다.
- [x] 설정은 bootstrap CLI option으로만 받으며 runtime UI/API로 변경하지 않는다.

## 비범위

- [x] Web Admin chart 대규모 개선하지 않음
- [x] runtime config editor 만들지 않음
- [x] raw log streaming 구현하지 않음
