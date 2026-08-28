# Sponzey Fleet 프로젝트 계획

작성일: 2026-06-04  
기반 문서: `RESEARCH.md`, 첨부 아이디어 메모  
목표: Rust core 기반 Controller/Agent/CLI와 가벼운 Web Admin UI를 npm 설치 UX로 배포하는 OSS Fleet Ops Platform 설계

## 1. 제품 정의

### 1.1 한 줄 컨셉

Sponzey Fleet는 Ansible처럼 서버를 자동화하되, agent 기반 실시간 연결로 실행, 상태 수집, 드리프트 감지, 로그 스트리밍, 자동 복구까지 제공하는 오픈소스 Fleet Ops Platform이다.

### 1.2 포지셔닝

나쁜 포지셔닝:

- SSH 안 쓰는 Ansible
- 또 하나의 구성 관리 도구
- Prometheus/Grafana 대체 모니터링

좋은 포지셔닝:

- Real-time server automation and observability platform
- Ansible for fleets that need real-time state
- Agent-based fleet automation, drift detection, and remediation
- 서버 자동화, 상태 감시, 드리프트 감지, 자동 복구를 하나로 묶은 OSS 운영 플랫폼

Sponzey Fleet는 Ansible, Salt, Puppet, Chef를 모두 대체하겠다는 제품이 아니다. 초기 제품은 다음 문제를 선명하게 해결한다.

- 운영자가 서버에 직접 SSH 접속하지 않고 안전하게 명령을 실행한다.
- NAT/private subnet/고객사 방화벽 뒤의 서버도 outbound agent 연결로 관리한다.
- 서버 상태와 작업 결과를 실시간으로 본다.
- 선언한 상태와 실제 상태가 달라졌는지 계속 확인한다.
- 자주 반복되는 운영 작업을 승인, 감사, 로그와 함께 표준화한다.

## 2. 유사 제품에서 가져올 장점

### 2.1 Ansible에서 가져올 것

- 사람이 읽기 쉬운 YAML 작업 정의
- `command`, `copy`, `template`, `package`, `service`, `user`, `file` 같은 명확한 primitive
- inventory, group, role 기반 대상 선택
- idempotent task 모델
- dry-run/check mode 개념
- Git으로 자동화 코드를 관리하는 흐름

가져오지 않을 것:

- Ansible playbook 전체 호환을 MVP 목표로 삼지 않는다.
- Jinja/YAML로 복잡한 프로그래밍을 강요하지 않는다.
- SSH fan-out 구조를 핵심 실행 모델로 두지 않는다.

### 2.2 Salt에서 가져올 것

- master/minion에서 검증된 agent 기반 대량 실행 모델
- 빠른 fan-out과 job return 구조
- event bus 기반 실시간 실행/상태 이벤트
- grains처럼 호스트 특성을 facts로 수집하고 targeting에 활용하는 방식
- state와 remote execution을 둘 다 지원하는 방향

차별화:

- Salt의 복잡한 transport/모듈 구조를 그대로 재현하지 않는다.
- 초기에는 WebSocket over TLS 또는 gRPC streaming 기반의 단순한 outbound 연결을 사용한다.

### 2.3 Puppet/Chef에서 가져올 것

- desired state와 catalog 개념
- agent가 facts를 보내고 controller가 정책/작업을 계산하는 모델
- drift 감지와 리포트
- no-op/dry-run 리포트
- compliance report와 audit trail

차별화:

- Puppet DSL이나 Chef Ruby cookbook처럼 무거운 DSL을 만들지 않는다.
- 초기 DSL은 Ansible-like YAML과 JSON schema 검증으로 제한한다.

### 2.4 Red Hat AAP/AWX에서 가져올 것

- Web Admin UI, REST API, CLI를 모두 제공하는 구조
- inventory, credential, job template, workflow, schedule
- RBAC와 audit log
- job output 실시간 표시
- 조직/프로젝트 단위 권한 분리

차별화:

- Kubernetes 필수 구조로 시작하지 않는다.
- 단일 Rust 바이너리, SQLite, npm install로 시작 가능한 경량성을 유지한다.
- Enterprise 기능은 나중에 HA/Postgres/multi-tenant로 확장한다.

### 2.5 Semaphore UI에서 가져올 것

- 가벼운 self-hosted 설치 경험
- Ansible뿐 아니라 Shell, Python, PowerShell, Terraform/OpenTofu까지 실행 가능한 확장성
- Community/Pro/Enterprise로 나누기 쉬운 기능 경계
- runner 기반 원격 실행 방향
- 공개 가격/투명한 제품 메시지

차별화:

- 단순 작업 실행 UI가 아니라 agent 기반 상태 감시와 drift/remediation을 제품 핵심으로 둔다.

### 2.6 Rundeck/PagerDuty Process Automation에서 가져올 것

- 운영자를 위한 runbook catalog
- 승인 단계와 수동 gate
- 실행 권한 위임
- job as code
- runner를 통한 private network 접근
- incident response와 remediation 흐름

차별화:

- 외부 Ansible 실행만 하는 런북 도구로 머무르지 않고, agent가 facts/metrics/logs/drift를 계속 제공한다.

### 2.7 GitHub Actions Runner에서 가져올 것

- self-hosted runner가 outbound HTTPS로 controller에 연결해 job을 받는 방식
- label/group 기반 job routing
- runner registration token
- 자동 업데이트 전략
- job pickup timeout과 requeue 모델

차별화:

- CI job runner가 아니라 서버 운영 task runner다.
- root 권한, 파일 변경, 서비스 재시작 같은 위험 작업에 approval, signed task, audit를 기본으로 둔다.

### 2.8 Elastic Fleet/FleetDM에서 가져올 것

- enrollment token 기반 agent 등록
- agent policy와 policy rollout
- token revoke
- agent check-in, health, last seen
- 패키지 생성 또는 one-line install command
- agent auto-update channel

차별화:

- 보안 데이터 수집 플랫폼이 아니라 운영 자동화 실행 플랫폼이다.
- osquery 같은 쿼리 엔진은 나중에 플러그인으로 붙인다.

### 2.9 OpenTelemetry Collector에서 가져올 것

- receiver, processor, exporter 형태의 telemetry pipeline 사고방식
- agent/collector 양쪽으로 배포 가능한 구조
- logs, metrics, traces 중 우선 metrics/logs subset
- 외부 backend로 export 가능한 구조

차별화:

- full observability platform이 아니라 자동화 판단에 필요한 최소 telemetry를 수집한다.
- Prometheus/Grafana를 대체하지 않고 export target으로 지원한다.

### 2.10 Nomad/Tailscale에서 가져올 것

- control plane과 data plane 분리
- client가 자신의 resources/attributes를 등록하고 controller가 placement/routing 판단
- node key, machine identity, policy 기반 접근 제어
- 대규모 client를 소수 controller가 관리하는 모델

차별화:

- workload scheduler가 아니라 운영 작업 실행 및 상태 관리 플랫폼이다.
- P2P mesh나 VPN을 만들지 않는다.

### 2.11 n8n/Node-RED에서 가져올 것

- npm global install로 시작하는 낮은 진입 장벽
- `npx`로 설치 없이 체험 가능한 quick start
- 플러그인/커뮤니티 확장을 npm package 또는 OCI artifact로 배포하는 모델
- 로컬 개발과 production 실행 경계를 명확히 나누는 문서

차별화:

- npm 설치는 개발자 친화 시작점이다. 코어 런타임은 Rust 바이너리이며, npm package는 플랫폼별 바이너리를 받아 설치하는 distribution wrapper로 둔다.
- production에서는 systemd/launchd/Windows Service 설치 명령과 `.deb`, `.rpm`, Homebrew, Docker image, standalone binary를 함께 제공해야 한다.

## 3. 제품 원칙

### 3.1 핵심 원칙

- Agent first: 서버는 controller로 outbound 연결한다.
- Secure by default: 등록, 명령, 결과, 로그는 처음부터 인증/암호화/감사 대상이다.
- Small primitives: 큰 DSL보다 검증 가능한 작은 task primitive를 제공한다.
- Real-time by default: job output, health, facts, drift result는 실시간으로 들어온다.
- Git friendly: 모든 task/policy/template은 Git에서 관리할 수 있어야 한다.
- Rust core: controller, agent, CLI, protocol, task runner의 핵심은 Rust로 구현한다.
- Lightweight Web Admin: UI는 별도 무거운 프론트엔드 플랫폼이 아니라 controller가 정적 파일로 서빙하는 가벼운 admin surface로 둔다.
- npm quick start: controller와 agent 모두 npm으로 설치할 수 있어야 한다.
- Production ready path: npm quick start 이후 서비스 등록, TLS, Postgres, reverse proxy, backup으로 이어지는 경로가 있어야 한다.

### 3.2 비협상 보안 기준

Sponzey Fleet는 root 권한 원격 실행 도구가 될 가능성이 높다. 따라서 보안은 후속 기능이 아니라 제품 전제다.

MVP부터 반드시 들어갈 것:

- agent enrollment token
- audit log
- command approval hook
- secret redaction
- signed task payload 설계
- least privilege mode 설계
- TLS 기본 통신
- agent fingerprint 저장
- 작업 timeout과 cancel

초기 구현에서 mTLS 전체 자동화를 완성하지 못하더라도, protocol과 저장 모델은 mTLS, certificate rotation, task signature를 나중에 무리 없이 붙일 수 있게 잡는다.

### 3.3 명시적으로 하지 않을 것

- Ansible 100% 호환
- Kubernetes operator부터 시작
- Prometheus/Grafana 대체
- EDR/보안 에이전트 포지션
- CI/CD 플랫폼 포지션
- 원격 쉘만 있는 단순 도구

## 4. 설치 및 패키징 전략

### 4.1 npm 패키지 구성

초기에는 단일 npm 패키지로 시작하되, 패키지 내부 구현은 JavaScript 애플리케이션이 아니라 플랫폼별 Rust 바이너리 설치 래퍼다.

```text
@sponzey/fleet
  bin:
    fleet
  optionalDependencies:
    @sponzey/fleet-darwin-arm64
    @sponzey/fleet-darwin-x64
    @sponzey/fleet-linux-x64
    @sponzey/fleet-linux-arm64
    @sponzey/fleet-win32-x64
```

명령 UX:

```bash
npm install -g @sponzey/fleet

fleet controller init
fleet controller start

fleet agent init --url https://fleet.example.com --token <enrollment-token>
fleet agent start
```

나중에 패키지를 분리할 수는 있지만, 실행 바이너리는 계속 단일 `fleet`를 기본으로 한다.

```text
@sponzey/fleet
@sponzey/fleet-plugin-sdk
@sponzey/fleet-web-admin
```

분리 기준:

- Plugin SDK나 Web Admin UI release train을 core binary와 분리해야 할 때
- Web Admin UI release train을 controller core와 분리해야 할 때
- 플러그인 생태계를 안정화할 때

Rust workspace 구조:

```text
crates/
  fleet-cli
  fleet-controller
  fleet-agent
  fleet-core
  fleet-protocol
  fleet-runner
  fleet-store
web-admin/
  src/
  dist/
```

배포 원칙:

- Rust workspace는 여러 crate로 나누지만 배포 바이너리는 `fleet` 하나다.
- Controller, Agent, CLI 역할은 `fleet controller ...`, `fleet agent ...`, `fleet ...` subcommand로 선택한다.
- `web-admin/dist`는 `fleet controller start`가 서빙하는 static asset으로 embed하거나 release artifact로 같이 패키징한다.
- npm package는 현재 OS/arch에 맞는 `fleet` Rust binary를 노출한다.
- npm이 없는 운영 환경은 standalone binary, `.deb`, `.rpm`, Homebrew, Docker를 사용한다.

### 4.2 npx 체험 모드

개발자 체험용:

```bash
npx @sponzey/fleet demo
```

동작:

- 임시 SQLite DB 생성
- localhost controller 실행
- local agent 1개 등록
- sample job 실행
- 브라우저 URL 출력

목표:

- 5분 안에 “agent가 붙고 명령 output이 스트리밍되는 경험”을 보여준다.

### 4.3 Controller 설치 모드

개발/PoC:

```bash
npm install -g @sponzey/fleet
fleet controller init --data-dir ./fleet
fleet controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir ./fleet \
  --external-url http://127.0.0.1:7700
```

운영:

```bash
npm install -g @sponzey/fleet
fleet controller init --data-dir /var/lib/fleet
fleet controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir /var/lib/fleet \
  --db sqlite:///var/lib/fleet/controller/fleet.db \
  --external-url https://fleet.example.com
```

운영 필수 옵션:

- Postgres
- TLS termination 또는 built-in TLS
- admin bootstrap token
- backup command
- log retention
- reverse proxy 문서

### 4.4 Agent 설치 모드

PoC:

```bash
npm install -g @sponzey/fleet
fleet agent init --url https://fleet.example.com --token <token>
fleet agent start
```

운영:

```bash
npm install -g @sponzey/fleet
sudo fleet agent init \
  --url https://fleet.example.com \
  --token <token> \
  --name web-01 \
  --labels role=web,env=prod,region=apne2

sudo fleet agent install-service
sudo fleet agent start-service
```

운영 고려:

- Linux MVP는 systemd 우선.
- macOS launchd, Windows Service는 후속.
- agent가 root로 실행될 수 있으므로 least privilege mode를 별도 제공한다.
- npm global install 경로와 root 실행 권한 충돌을 문서화한다. 운영에서는 npm 설치 후 서비스 파일이 Rust 바이너리의 절대 경로를 참조하도록 고정한다.
- 장기적으로 `.deb`, `.rpm`, Homebrew, Docker, standalone binary를 1급 설치 경로로 제공한다.

### 4.5 One-line install

npm이 없는 서버를 위해 bootstrap script도 제공한다.

```bash
curl -fsSL https://get.fleet.dev/fleet.sh | sudo bash -s -- \
  --url https://fleet.example.com \
  --token <token> \
  --labels role=web,env=prod
```

이 스크립트는 다음만 수행한다.

- OS/arch 확인
- Rust standalone binary 또는 npm 설치 경로 선택
- version/checksum/signature 검증
- `@sponzey/fleet` 또는 standalone archive 설치
- agent init
- service install/start

보안상 스크립트는 checksum, version pinning, dry-run 옵션을 제공해야 한다.

### 4.6 버전 및 업데이트

채널:

- `latest`: 안정 버전
- `next`: 다음 minor 후보
- `edge`: nightly/실험 기능

업데이트 명령:

```bash
fleet controller upgrade --to latest
fleet agent upgrade --channel latest
```

MVP에서는 자동 업데이트를 하지 않는다. v0.3 이후 agent policy 기반 staged rollout을 지원한다.

## 5. 시스템 아키텍처

### 5.1 논리 구조

```text
Sponzey Controller
  API Server
  Web Admin UI
  Agent Gateway
  Job Scheduler
  Inventory DB
  Policy Engine
  Drift Engine
  Telemetry Receiver
  Audit Log

Sponzey Agent
  Secure Tunnel Client
  Task Runner
  Facts Collector
  Metrics Collector
  Log Tailer
  Drift Checker
  Package Manager Adapter
  Service Manager Adapter
  File/Template Manager
```

### 5.2 통신

MVP 권장:

- WebSocket over TLS
- Agent outbound only
- Rust `serde` 기반 JSON messages + protobuf-compatible schema 설계
- heartbeat, task assignment, output stream, result, telemetry stream 채널 분리

v0.3 이후:

- gRPC streaming 선택 지원
- mTLS
- agent certificate rotation
- controller HA gateway

### 5.3 데이터 저장소

MVP:

- SQLite 기본
- Postgres 선택
- local file artifact storage
- SQLx migration 또는 SeaORM migration

운영:

- Postgres 필수 권장
- S3-compatible artifact storage
- structured audit event table
- job log retention policy

### 5.4 보안 모델

처음부터 포함:

- enrollment token
- agent key pair
- controller-signed agent identity
- signed task payload
- output secret redaction
- audit event append-only model
- command approval hook
- agent capability 선언

후속:

- mTLS
- Vault/OpenBao integration
- OIDC/LDAP/SAML
- project/team RBAC
- policy-as-code
- FIPS/air-gapped packaging

## 6. 핵심 객체 모델

### 6.1 Agent

필드:

- `id`
- `name`
- `fingerprint`
- `labels`
- `os`
- `arch`
- `version`
- `capabilities`
- `last_seen_at`
- `status`
- `policy_id`

상태:

- `pending`
- `online`
- `busy`
- `degraded`
- `offline`
- `disabled`

### 6.2 Inventory

대상 선택:

```text
agent:web-01
label:role=web
label:env=prod
group:production-web
query:os.name == "ubuntu" and metrics.disk.root.used_pct > 80
```

초기에는 label selector만 지원하고, query selector는 v0.4 이후로 미룬다.

### 6.3 Job

필드:

- `id`
- `name`
- `source`
- `target_selector`
- `tasks`
- `status`
- `created_by`
- `approved_by`
- `started_at`
- `finished_at`
- `result_summary`

상태:

- `draft`
- `pending_approval`
- `queued`
- `running`
- `partial_success`
- `success`
- `failed`
- `canceled`
- `expired`

### 6.4 Task

초기 primitive:

- `command`
- `shell`
- `file.copy`
- `file.template`
- `package`
- `service`
- `user`
- `group`
- `cron`
- `port.check`
- `process.check`
- `reboot`
- `facts.collect`
- `logs.tail`
- `metrics.snapshot`

### 6.5 Policy

정책 예시:

```yaml
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: nginx-running
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: nginx-service
      service:
        name: nginx
        state: running
  remediation:
    approval: manual
    tasks:
      - service:
          name: nginx
          state: restarted
```

### 6.6 Drift

드리프트 결과:

```yaml
agent: web-01
policy: nginx-baseline
status: drifted
expected:
  service.nginx.enabled: true
  file./etc/nginx/nginx.conf.sha256: abc123
actual:
  service.nginx.enabled: false
  file./etc/nginx/nginx.conf.sha256: def456
detectedAt: 2026-06-04T09:00:00Z
```

## 7. DSL 설계

### 7.1 MVP YAML

```yaml
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
metadata:
  name: setup-nginx
spec:
  targets:
    selector:
      matchLabels:
        role: web
  strategy:
    concurrency: 10
    maxFailures: 1
  tasks:
    - name: install nginx
      package:
        name: nginx
        state: present
    - name: render config
      template:
        src: nginx.conf.tpl
        dest: /etc/nginx/nginx.conf
        mode: "0644"
    - name: restart nginx
      service:
        name: nginx
        state: restarted
        enabled: true
```

### 7.2 DSL 원칙

- YAML은 선언적이어야 한다.
- 각 task는 JSON schema로 검증한다.
- task output은 구조화한다.
- idempotent 가능한 task는 `changed: true/false`를 반환한다.
- command/shell은 기본적으로 unsafe로 분류한다.
- template은 초기에는 Mustache/Handlebars 계열로 제한한다.
- 복잡한 조건문은 v0.2 이후에 최소 지원한다.

### 7.3 Ansible import

초기에는 하지 않는다.

v0.5 후보:

```bash
fleet import ansible site.yml --output fleet-runbook.yml
```

지원 범위:

- `hosts`
- `tasks`
- `command`
- `shell`
- `copy`
- `template`
- `apt/yum/dnf`
- `service`
- `user`

지원하지 않을 것:

- role 전체 호환
- 복잡한 variable precedence
- 모든 module
- dynamic inventory 전체 호환

## 8. MVP

### 8.1 MVP 목표

MVP는 “npm으로 controller와 agent를 설치하고, agent가 outbound로 붙고, UI/CLI에서 안전하게 명령을 실행하며, facts/metrics/logs/drift의 최소 루프를 보여주는 것”이다.

성공 기준:

- 10분 안에 로컬 controller와 agent 1개 실행
- 30분 안에 원격 Linux 서버 3대 enroll
- `uptime`, `df -h`, `systemctl status nginx` 같은 명령을 UI/CLI에서 실시간 출력
- labels로 대상 선택
- 기본 facts와 metrics 표시
- nginx running policy drift 감지
- 모든 실행이 audit log에 남음

### 8.2 MVP 기능

#### 설치

- `npm install -g @sponzey/fleet`
- `npx @sponzey/fleet demo`
- `fleet controller init/start`
- `fleet agent init/start`
- `fleet agent install-service` for systemd

#### Controller

- REST API
- OpenAPI 3.1 JSON과 Swagger UI 기반 외부 API 문서
- WebSocket Agent Gateway
- SQLite DB
- minimal Web Admin UI
- enrollment token 발급/폐기
- inventory/agent list
- job create/run/cancel
- job output stream 저장
- audit log 저장

#### Agent

- outbound WebSocket 연결
- heartbeat
- enrollment
- command 실행
- stdout/stderr streaming
- facts 수집
- metrics snapshot
- service/package/file primitive 일부
- local log tail

#### CLI

- `fleet login`
- `fleet agents list`
- `fleet fleet list`
- `fleet enroll-token create`
- `fleet agent install`
- `fleet run --selector role=web "uptime"`
- `fleet apply playbook.yml`
- `fleet facts web-01`
- `fleet logs nginx`
- `fleet logs web-01 --file /var/log/syslog`
- `fleet metrics web-01`
- `fleet drift check --policy nginx-running`

초기 명령 alias:

| 명령                           | 의미                                      | 실제 내부 동작                                                       |
| ---------------------------- | --------------------------------------- | -------------------------------------------------------------- |
| `fleet agent install`      | agent 설치/등록/서비스화를 한 번에 처리               | `agent init` + `agent install-service` + `agent start-service` |
| `fleet fleet list`         | fleet 대상 서버 목록 조회                       | `agents list`                                                  |
| `fleet run "uptime"`       | 기본 selector 전체 또는 현재 context 대상으로 명령 실행 | `run --selector <context> "uptime"`                            |
| `fleet apply playbook.yml` | YAML runbook 적용                         | `runbook apply playbook.yml`                                   |
| `fleet facts`              | 현재 context 대상 facts 조회                  | `facts --selector <context>`                                   |
| `fleet logs nginx`         | 서비스 로그 shortcut                         | systemd/journald 또는 configured log source tail                 |
| `fleet metrics`            | 현재 context metrics snapshot             | `metrics --selector <context>`                                 |
| `fleet drift check`        | policy 기준 drift 검사                      | `drift check --policy <default>`                               |

#### UI

- 로그인 없는 local bootstrap 또는 admin token 방식
- agents table
- agent detail
- run command form
- live output viewer
- facts/metrics view
- job history
- audit event list

### 8.3 MVP에서 제외

- Multi-tenant
- HA controller
- Windows agent
- full RBAC
- SAML/LDAP
- Vault integration
- Ansible import
- plugin marketplace
- auto remediation without approval
- full metrics time-series DB
- Kubernetes deployment

### 8.4 MVP 이후 Phase 002 목표

MVP 이후 첫 제품화 목표는 “다른 노트북/서버의 agent가 SSH tunnel 없이 안전하게 controller에 붙는 remote beta”다. HTTP는 기술적으로 허용하지만 테스트 전용 transport로 취급하고, 사용 시 항상 명확한 경고와 audit를 남긴다. 제품, 고객, 운영, 공동 사용, 장시간 실행 환경의 정식 경로는 HTTPS/TLS 기반 enrollment와 WSS 기반 agent channel이다.

Phase 002의 핵심 방향:

- controller bind address와 external URL을 분리한다.
- `http://127.0.0.1`과 LAN HTTP URL은 테스트 편의상 허용하되 test-only warning을 유지한다.
- 원격 production agent는 `https://fleet.example.com` 같은 HTTPS URL로 등록한다.
- controller signing identity pinning과 TLS certificate trust를 분리한다.
- enrollment token은 scope, expiry, single-use, revoke, audit를 갖춘다.
- agent production service lifecycle을 systemd 기준으로 정리한다.
- Web Admin UI에서 enrollment token, agent health, approval, audit를 다룬다.
- packaging은 npm wrapper뿐 아니라 standalone binary와 Linux compatibility gate를 포함한다.

세부 실행 계획은 `.tasks/plan.md`와 `.tasks/task001.md`부터 `.tasks/task009.md`를 따른다.

## 9. 개발 계획

각 단계는 2~3개 기능만 묶는다. 단계가 끝날 때마다 사용 가능한 제품 조각이 남아야 한다.

### Phase 0. Rust 프로젝트 골격

기능:

1. Rust workspace와 CLI entrypoint 구성
2. Controller/Agent 공통 protocol crate 정의
3. SQLite schema와 migration 기본 구조

완료 기준:

- `npm install -g` 후 `fleet --help` 동작
- `fleet controller start`가 빈 API 서버 실행
- protocol message type이 Rust type과 문서로 정의됨

산출물:

- `@sponzey/fleet` 패키지
- Rust workspace
- 기본 CLI
- `docs/protocol.md`

### Phase 1. Controller와 Agent 연결

기능:

1. enrollment token 생성/검증
2. agent outbound WebSocket 연결과 heartbeat
3. agent list/last seen API

완료 기준:

- `fleet enroll-token create`로 토큰 발급
- `fleet agent init --url ... --token ...` 성공
- UI/CLI에서 online agent 확인

핵심 설계:

- 등록 토큰은 단기 secret이다.
- 등록 후 agent는 자체 key pair를 생성한다.
- controller는 agent fingerprint를 저장한다.

### Phase 2. Remote Run과 Live Output

기능:

1. `command` task 실행
2. stdout/stderr 실시간 스트리밍
3. job history와 result 저장

완료 기준:

- `fleet run --selector role=web "uptime"` 동작
- 서버별 output이 CLI와 UI에 실시간 표시
- 실패/성공/타임아웃 상태가 저장

보안 기준:

- shell command는 audit에 원문 저장
- secret redaction 기본 패턴 적용
- 기본 timeout 필수

### Phase 3. Facts와 Inventory

기능:

1. OS/CPU/memory/disk/network facts 수집
2. label 기반 inventory selector
3. agent detail UI

완료 기준:

- `fleet facts web-01` 출력
- `--selector role=web,env=prod` 대상 선택
- UI에서 agent facts와 labels 편집 가능

### Phase 4. Task Primitives 1차

기능:

1. `package`, `service`, `file.copy`
2. Runbook YAML parser와 JSON schema validation
3. dry-run 지원 기반

완료 기준:

- nginx 설치/시작 runbook 실행
- 지원하지 않는 DSL은 명확한 validation error
- package/service task는 `changed` 여부 반환

### Phase 5. Web Admin UI MVP

기능:

1. agents/job/runbook 화면
2. live output viewer
3. audit log 화면

완료 기준:

- 브라우저에서 agent 선택 후 명령 실행
- job detail에서 서버별 output 확인
- 누가 어떤 작업을 실행했는지 audit 확인

### Phase 6. Metrics와 Logs 최소 기능

기능:

1. CPU/memory/disk/process/service metrics snapshot
2. log tail streaming
3. metrics/logs retention 설정

완료 기준:

- `fleet metrics web-01` 출력
- `fleet logs web-01 --file /var/log/syslog` streaming
- UI에서 최근 metrics snapshot 확인

주의:

- time-series database를 직접 만들지 않는다.
- Prometheus/OpenTelemetry export는 후속으로 둔다.

### Phase 7. Drift Detection

기능:

1. desired state policy 정의
2. agent-side check 실행
3. drift result 저장/표시

완료 기준:

- nginx enabled/running policy 작성
- 수동 변경 후 drift 감지
- expected/actual diff가 UI/CLI에 표시

### Phase 8. Approval과 안전장치

기능:

1. dangerous task 분류
2. manual approval workflow
3. signed task payload

완료 기준:

- `shell`, `reboot`, `service.restart`는 승인 요구 가능
- 승인자와 실행자가 audit에 분리 기록
- agent는 controller 서명 없는 task 거부

### Phase 9. Service Install과 운영 배포

기능:

1. systemd service install/uninstall
2. controller backup/restore
3. Postgres 운영 모드

완료 기준:

- agent/controller 재부팅 후 자동 시작
- SQLite에서 Postgres로 운영 가이드 제공
- backup/restore smoke test 통과

### Phase 10. Template과 File Management

기능:

1. `template` primitive
2. file checksum drift
3. artifact storage

완료 기준:

- template render 후 remote file 배포
- checksum drift 감지
- job artifact와 rendered file 기록

### Phase 11. Policy-based Remediation

기능:

1. drift policy에 fix task 연결
2. remediation approval
3. remediation result report

완료 기준:

- nginx stopped 감지 후 restart 제안
- 승인 후 자동 fix 실행
- fix 전/후 결과가 report에 남음

### Phase 12. External Integrations 1차

기능:

1. Slack/Teams webhook notification
2. Prometheus/OpenTelemetry export
3. Git repository runbook sync

완료 기준:

- job success/failure 알림
- metrics snapshot 외부 export
- Git에서 runbook import/sync

### Phase 13. Enterprise 기반

기능:

1. OIDC 로그인
2. project/team RBAC
3. audit export

완료 기준:

- Google/GitHub/Keycloak OIDC login
- project별 agent/job 접근 제한
- audit CSV/JSON export

### Phase 14. Cross-platform Agent

기능:

1. Windows agent service
2. PowerShell task primitive
3. macOS launchd support

완료 기준:

- Windows Server에서 enroll/run 동작
- PowerShell command output streaming
- macOS agent service 설치 가능

### Phase 15. Ansible Bridge

기능:

1. Ansible-like runbook import subset
2. Ansible execution adapter
3. migration report

완료 기준:

- 단순 Ansible playbook을 Fleet runbook으로 변환
- 변환 불가 task를 report
- 필요 시 agent에서 `ansible-playbook` adapter 실행

## 10. 기술 스택 제안

### 10.1 초기 스택

언어/런타임:

- Rust core
- Node.js는 Web Admin UI 빌드와 npm 배포 wrapper에만 사용

Rust workspace:

- `fleet-core`: 공통 domain model, errors, config
- `fleet-protocol`: agent-controller message schema
- `fleet-controller`: `fleet controller`가 사용하는 API server, agent gateway, scheduler library
- `fleet-agent`: `fleet agent`가 사용하는 daemon, task runner, facts/metrics/logs/drift collector library
- `fleet-runner`: command/package/service/file primitive 실행
- `fleet-store`: SQLite/Postgres storage abstraction
- `fleet-cli`: 단일 제품 바이너리 `fleet`와 subcommand UX

Controller:

- Axum. Controller HTTP/WebSocket layer는 Axum으로 전환하는 것을 공식 방향으로 둔다. Tower middleware 생태계, Tokio 친화성, 테스트 용이성, 얇은 handler 유지 측면에서 Actix보다 MVP 이후 구조에 더 적합하다.
- Tokio async runtime
- WebSocket over TLS
- SQLite 기본, Postgres 운영
- SQLx 또는 SeaORM
- Tower middleware
- tracing 기반 structured logging
- Web Admin UI static asset serving

Agent:

- Rust daemon
- Tokio task runtime
- `std::process`/`tokio::process` 기반 task runner
- OS별 facts/metrics collector
- journald/syslog/file log tail adapter
- systemd adapter MVP
- Windows service adapter는 후속

CLI:

- clap
- local config in `~/.fleet/config.toml`
- shell completion

Web Admin UI:

- React/Vite 또는 SvelteKit static export
- TypeScript는 UI에만 사용
- Controller가 `/admin` 아래에서 정적 파일로 서빙
- 초기에는 SPA routing과 REST/WebSocket client만 제공
- UI는 제품 핵심이 아니라 운영자가 빠르게 상태 확인과 실행을 하는 얇은 admin layer로 유지
- 서버 사이드 렌더링, 별도 Node.js web server, 복잡한 디자인 시스템은 MVP에서 제외
- 화면은 agents, jobs, run command, live output, facts/metrics, drift, audit에만 집중

Protocol:

- JSON over WebSocket MVP
- message schema는 Rust `serde` type과 JSON Schema export로 검증
- v0.3 이후 protobuf/gRPC 고려

### 10.2 Rust core 선택의 장점

- agent가 장기 실행 daemon으로 안정적이다.
- root 권한 task runner를 더 작은 runtime과 명확한 ownership 모델로 구현할 수 있다.
- 단일 바이너리 배포가 쉽다.
- Linux systemd, Windows service, macOS launchd 같은 네이티브 서비스 통합에 유리하다.
- 메모리 사용량과 cold start가 Node daemon보다 예측 가능하다.
- protocol, controller, agent, CLI를 같은 type system으로 묶을 수 있다.
- 추후 `.deb`, `.rpm`, Homebrew, Docker, air-gapped archive 배포가 자연스럽다.

### 10.3 Rust core 선택의 약점

- 초기 개발 속도는 TypeScript 단일 스택보다 느릴 수 있다.
- Web Admin UI와 core 사이에 API/schema 동기화가 필요하다.
- 플러그인 생태계는 npm만큼 바로 열기 어렵다.
- cross compile, code signing, platform-specific service install을 릴리즈 파이프라인에서 챙겨야 한다.

대응:

- Web Admin UI는 TypeScript로 빠르게 만들고, core API schema를 자동 export한다.
- npm install은 유지하되 npm은 Rust binary distribution channel로만 사용한다.
- plugin은 초기에는 외부 executable adapter와 YAML task primitive로 제한한다.
- v0.4 이후 WASI plugin 또는 sidecar plugin protocol을 검토한다.

## 11. 보안 요구사항

### 11.1 MVP 필수

- enrollment token은 생성 시 1회만 노출
- token revoke 지원
- agent fingerprint 저장
- controller URL pinning
- task timeout 기본값
- command allow/deny pattern
- secret redaction
- audit log append-only 설계
- local agent config file permission check
- dangerous task approval hook

### 11.2 v0.3 이상 필수

- mTLS
- signed task
- agent certificate rotation
- per-project RBAC
- OIDC login
- vault integration
- remote script checksum validation
- agent auto-update policy

### 11.3 위험 작업 분류

위험도 `low`:

- facts collect
- metrics snapshot
- service status
- port check
- process check
- log tail read-only

위험도 `medium`:

- package install/update
- file copy/template
- service restart
- user/group create

위험도 `high`:

- shell command
- reboot
- file delete
- permission recursive change
- package remove
- arbitrary script execution

기본 정책:

- high는 승인 없이는 실행 불가로 설정할 수 있어야 한다.
- production label이 붙은 agent에는 stricter policy를 적용할 수 있어야 한다.

## 12. 제품/수익화 계획

### 12.1 OSS Edition

포함:

- single controller
- npm install
- Linux agent
- CLI
- basic Web Admin UI
- command/runbook execution
- facts/metrics/logs basics
- drift check
- SQLite/Postgres
- basic audit log

### 12.2 Team Edition

포함:

- OIDC
- team/project RBAC
- approval workflow
- Postgres 운영 지원
- audit export
- Slack/Teams notification
- Git sync

### 12.3 Enterprise Edition

포함:

- HA controller
- multi-tenant
- SAML/LDAP
- air-gapped package
- Vault/OpenBao/CyberArk integration
- compliance report
- Windows support
- policy packs
- priority support
- migration support from AWX/Semaphore/Rundeck

### 12.4 서비스 매출

- 자동화 runbook 개발
- 패치/컴플라이언스 policy pack
- 온프레미스/폐쇄망 구축
- MSP 운영 대행
- 기존 Ansible/AWX/Semaphore 마이그레이션

## 13. 경쟁 우위

### 13.1 핵심 차별점

- npm으로 시작 가능한 agent/controller
- SSH 접근 없이 private network 서버 관리
- 실시간 output streaming
- facts/metrics/logs/drift를 한 agent가 제공
- runbook 실행과 policy remediation이 같은 플랫폼 안에 있음
- Ansible-like YAML로 익숙한 사용성
- 운영 자동화에 필요한 최소 observability만 제공

### 13.2 비교표

| 항목      | Ansible   | AWX/AAP              | Semaphore  | Rundeck           | Salt    | Sponzey Fleet  |
| ------- | --------- | -------------------- | ---------- | ----------------- | ------- | -------------- |
| 기본 연결   | SSH/WinRM | SSH/실행 환경            | SSH/runner | SSH/runner/plugin | minion  | outbound agent |
| 설치 난도   | 낮음        | 중~높음                 | 낮음         | 중간                | 중간      | 낮음 목표          |
| 실시간 상태  | 약함        | 제한적                  | 제한적        | 작업 중심             | 강함      | 핵심 기능          |
| 드리프트 감지 | 별도 구현     | 제한적                  | 제한적        | 제한적               | 가능      | 핵심 기능          |
| npm 설치  | 아님        | 아님                   | 아님         | 아님                | 아님      | 핵심 UX          |
| 운영 UI   | 없음        | 강함                   | 중간         | 강함                | 다양      | MVP부터          |
| 모니터링    | 없음        | 제한적                  | 제한적        | 제한적               | 이벤트 중심  | 최소 내장          |
| 자동 복구   | 직접 구현     | Event-Driven Ansible | 제한적        | 가능                | reactor | policy 기반      |

## 14. 초기 런칭 메시지

README 상단 후보:

```text
# Sponzey Fleet

An open-source agent-based fleet automation platform.

Run commands, apply configuration, detect drift, stream logs,
collect metrics, and safely remediate Linux servers from one controller.

Install the controller and agent with npm:

  npm install -g @sponzey/fleet
```

짧은 문구:

- I wanted Ansible, but always connected.
- Real-time automation for Linux fleets.
- Run, observe, detect drift, remediate.
- Fleet automation without inbound SSH.

## 15. 첫 공개 데모 시나리오

### Demo 1. Local quick start

```bash
npx @sponzey/fleet demo
```

보여줄 것:

- controller 시작
- local agent 자동 등록
- UI 접속
- `uptime` 실행
- live output
- facts/metrics 표시

### Demo 2. Remote Linux fleet

```bash
npm install -g @sponzey/fleet
fleet controller init
fleet controller start
fleet enroll-token create --labels env=dev
```

원격 서버:

```bash
npm install -g @sponzey/fleet
sudo fleet agent init --url https://fleet.example.com --token <token> --labels role=web
sudo fleet agent install-service
```

보여줄 것:

- agent 3대 online
- label selector run
- output streaming
- job history

### Demo 3. Drift and remediation

```bash
fleet policy apply nginx-running.yml
fleet drift check --selector role=web
```

보여줄 것:

- nginx 중지
- drift 감지
- remediation 제안
- 승인 후 restart
- report 생성

## 16. 성공 지표

MVP adoption:

- `npx @sponzey/fleet demo` 성공률 95% 이상
- fresh Linux VM에서 agent 등록까지 5분 이하
- controller+agent npm 설치 이슈 10% 이하
- 100 agent 동시 heartbeat 안정
- 1,000 command job output streaming 안정

제품 가치:

- 수동 SSH 작업 감소
- 반복 운영 작업 표준화
- 장애 대응 시간 감소
- 감사 로그 자동화
- drift 발견 시간 단축

OSS 지표:

- GitHub stars
- npm weekly downloads
- active agents in telemetry opt-in
- community runbook submissions
- issue response time

## 17. 참고 자료

- `RESEARCH.md`
- OpenTelemetry Collector documentation: https://opentelemetry.io/docs/collector/
- GitHub self-hosted runners reference: https://docs.github.com/actions/reference/runners/self-hosted-runners
- Puppet agent documentation: https://help.puppet.com/pe/current/topics/the_puppet_agent.htm
- Puppet run lifecycle: https://help.puppet.com/core/current/Content/PuppetCore/details_about_puppets_internals.htm
- Salt system architecture: https://docs.saltproject.io/en/master/topics/salt_system_architecture.html
- Salt event system: https://docs.saltproject.io/en/master/topics/event/events.html
- Chef Infra Server overview: https://docs.chef.io/server/
- n8n npm installation: https://docs.n8n.io/hosting/installation/npm/
- Node-RED creating nodes: https://nodered.org/docs/creating-nodes/
- Elastic Fleet enrollment tokens: https://www.elastic.co/guide/en/fleet/current/fleet-enrollment-tokens.html
- FleetDM enroll hosts: https://fleetdm.com/guides/enroll-hosts
- Nomad architecture: https://developer.hashicorp.com/nomad/docs/architecture
- Tailscale control and data planes: https://tailscale.com/docs/concepts/control-data-planes