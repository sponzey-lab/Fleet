# Sponzey Fleet

[English](README.md)

Sponzey Fleet는 여러 서버나 노트북을 한 곳에서 관리하기 위한 agent 기반 운영 자동화 도구입니다. 실행 파일은 하나뿐입니다. 이름은 `sponzey`이고, 실행하는 명령에 따라 controller가 되기도 하고 agent가 되기도 합니다.

```text
sponzey controller ...
sponzey agent ...
sponzey enroll-token ...
sponzey run ...
sponzey demo
```

핵심 런타임은 Rust입니다. npm 패키지는 Rust 바이너리를 설치하기 위한 도구일 뿐입니다.

## 아주 간단한 그림

| 구분         | 어디서 실행하나                  | 하는 일                                                                        |
| ---------- | ------------------------- | --------------------------------------------------------------------------- |
| Controller | 관리자가 브라우저로 접속하는 컴퓨터 또는 서버 | DB를 저장하고, Web Admin UI를 열어주고, agent 등록 token을 만들고, agent 연결을 받고, 작업에 서명합니다. |
| Agent      | 관리 대상 컴퓨터 또는 서버마다 하나씩     | controller에 접속하고, health/facts/metrics를 보내고, controller가 서명한 작업을 실행합니다.     |

Controller 하나에 agent 여러 대가 붙습니다.

헷갈리기 쉬운 단어:

| 단어               | 뜻                                                             |
| ---------------- | ------------------------------------------------------------- |
| Data directory   | Sponzey가 key, DB, 로컬 설정을 저장하는 폴더입니다. 로컬 예시는 `.sponzey`를 씁니다.  |
| Admin token      | `sponzey controller init`이 출력합니다. Web Admin UI와 보호 API에만 씁니다. |
| Enrollment token | `sponzey enroll-token create`가 출력합니다. agent를 등록할 때 한 번 씁니다.   |
| Controller URL   | agent가 controller에 접속할 주소입니다. URL이 로컬이든 HTTPS든 설정 순서는 같습니다.   |

Facts와 Metrics는 서로 다른 데이터입니다.

| 데이터     | 뜻                                                                                                             |
| ------- | ------------------------------------------------------------------------------------------------------------- |
| Facts   | 거의 변하지 않는 인벤토리입니다. OS, 아키텍처, hostname, CPU 코어 수, 메모리 총량/모듈 수, 디스크 장치 수, 마운트 구조, 디스크 총 용량, 네트워크 인터페이스 등을 담습니다. |
| Metrics | 시간에 따라 변하는 사용량입니다. CPU 사용률, 메모리 사용량, 디스크 사용량, 프로세스 수, 실패한 서비스 수 등을 담습니다.                                      |

## 설치

```bash
npm install -g @sponzey/fleet
sponzey --help
```

설치 후 `sponzey` 명령을 찾지 못하면 npm global bin 경로가 `PATH`에
없는 상태입니다. 설치 스크립트는 가능한 경우 npm global `sponzey`
launcher를 만들고, `/usr/local/bin`처럼 안전하고 쓰기 가능한 `PATH`
안의 bin 디렉토리에도 launcher 생성을 시도합니다. 설치 스크립트가 shell
profile 파일을 조용히 수정하지는 않습니다. 그래도 shell이 `sponzey`를
찾지 못하면 먼저 npm bin 경로를 확인합니다.

```bash
echo "$(npm prefix -g)/bin"
```

그 경로를 shell `PATH`에 추가합니다.

```bash
export PATH="$(npm prefix -g)/bin:$PATH"
```

이 저장소에서 직접 실행하려면:

```bash
cargo build -p fleet-cli
./target/debug/sponzey --help
```

소스 빌드를 쓰는 경우 아래 예시의 `sponzey`를 `./target/debug/sponzey`로 바꾸면 됩니다.

### 설치 경로 선택

개발 환경이나 작은 서버에서는 npm 설치가 가장 단순합니다.

```bash
npm install -g @sponzey/fleet
```

대상 host에 npm을 두고 싶지 않다면 standalone release archive를 사용합니다.
release archive 이름은 다음 규칙을 따릅니다.

```text
sponzey-darwin-arm64.tar.gz
sponzey-darwin-x64.tar.gz
sponzey-linux-arm64.tar.gz
sponzey-linux-x64.tar.gz
```

설치 전에 checksum을 확인합니다.

```bash
./scripts/verify_standalone_artifacts.sh dist/release
```

release workflow는 archive와 함께 `SHA256SUMS`를 게시합니다. signature
검증은 아직 구현하지 않았으므로, 현재 integrity 경계는 checksum 검증과
release provenance 확인입니다.

장시간 실행하는 Linux host에서는 resolved binary를 systemd service로
등록합니다.

```bash
sponzey controller install-service --data-dir /var/lib/sponzey-fleet --dry-run
sudo sponzey controller install-service --data-dir /var/lib/sponzey-fleet
sponzey controller status-service --dry-run
sponzey controller logs-service --dry-run
```

Agent service도 `sponzey agent install-service` 형태로 동일하게 사용합니다.
service unit은 명시 CLI 인자를 사용하며 runtime에 process environment를
patch하지 않습니다.

upgrade는 현재 외부 package 또는 artifact 교체 작업입니다. binary를
교체하기 전 정책을 먼저 확인합니다.

```bash
sponzey upgrade --dry-run
```

controller storage에 영향을 줄 수 있는 upgrade 전에는 controller data를
backup해야 합니다.

## 가장 빠른 데모

```bash
sponzey demo
```

임시 controller를 띄우고, 임시 agent를 등록하고, sample job을 실행한 뒤 Web Admin URL을 출력합니다.

## API 문서

Controller는 운영자 화면을 `/admin`에서 제공합니다. 외부 REST API 문서는
OpenAPI 3.1 JSON과 Swagger UI로 제공합니다.

```text
GET /openapi.json
GET /swagger-ui
```

보호 API를 호출할 때는 `sponzey controller init`이 출력한 admin token을
Bearer token으로 사용합니다. HTTP에서 Swagger UI를 쓰면 token과 request
payload가 암호화되지 않으므로 로컬 또는 짧은 테스트 용도로만 사용해야
합니다. 상세 API 계약, public/internal endpoint 경계, pagination 형태,
deprecation policy는 [docs/api.md](docs/api.md)에 유지합니다. agent WebSocket
protocol은 [docs/protocol.md](docs/protocol.md)에 별도로 문서화합니다.

현재 bootstrap admin token은 `bootstrap-admin` actor와 `owner` role로
매핑됩니다. 최소 role/permission 경계는 [docs/security.md](docs/security.md)에
정리합니다.

현재 구현 상태는 [docs/feature-matrix.md](docs/feature-matrix.md)에
정리합니다. release 검증 명령과 필수 smoke check는
[docs/release-gate.md](docs/release-gate.md)에 정리합니다.

## Transport 안전 경고

HTTP controller URL은 설치 확인, 로컬 개발, 실험실 테스트, 짧은 검증 용도로만
지원합니다. HTTP는 반드시 테스트 전용 transport로 취급해야 합니다.

제품, 고객, 운영, 공동 사용, 장시간 실행 환경에서는 반드시 HTTPS를 사용해야
합니다. HTTP로 Sponzey를 실행하면 controller-agent 통신이 암호화되지
않습니다. HTTP transport는 기밀성이나 무결성 보장을 제공하지 않으며 token,
command, 운영 데이터, traffic이 노출되거나 중간자 공격을 받을 수 있습니다.

## 먼저 값만 정하기

설정 순서는 항상 같습니다. 아래 예시는 먼저 그대로 복사해서 확인할 수
있도록 로컬 값으로 되어 있습니다.

```text
DATA_DIR:        .sponzey
CONTROLLER_URL: http://127.0.0.1:7700
```

실제 원격 controller로 옮길 때는 값만 바꿉니다.

- data directory는 `/var/lib/sponzey-fleet` 같은 운영용 경로를 씁니다.
- controller URL은 `http://192.168.0.10:7700` 또는 `https://fleet.example.com` 같은 주소를 씁니다.
- `http://`는 테스트 용도로만 사용합니다. 제품 또는 운영 환경에서는 `https://`를 사용합니다.
- controller URL이 `http://`로 시작하면 controller-agent 통신이 암호화되지 않으므로 Sponzey가 실행할 때마다 경고를 출력합니다.
- HTTPS를 쓰려면 먼저 [HTTPS 준비](#https-준비)를 끝내면 됩니다.

## 하나의 설정 흐름

로컬 테스트, SSH tunnel 개발, 테스트 전용 HTTP 원격 사용, HTTPS 원격 사용
모두 순서는 같습니다. 여기의 명령은 로컬에서 먼저 복사해 실행해보는
버전입니다. 실제 원격 controller로 사용할 때는 data directory, controller
URL, 이름, label, token 값만 바꿉니다.

### 1. Controller 초기화

Controller 컴퓨터에서 처음 한 번 실행합니다.

```bash
sponzey controller init --data-dir .sponzey
```

이 명령이 출력하는 `admin token`을 복사해두세요. Web Admin UI에 붙여넣습니다.

### 2. Controller 시작

```bash
sponzey controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir .sponzey \
  --external-url http://127.0.0.1:7700
```

Controller 터미널은 계속 켜둡니다.

### 3. Web Admin 열기

Controller URL 뒤에 `/admin`을 붙여 엽니다.

```text
http://127.0.0.1:7700/admin
```

1단계에서 복사한 admin token을 붙여넣습니다.

Web Admin 화면에서는 agent inventory, 선택한 agent 상세, facts, disk/mount
inventory, range selector가 있는 metrics chart, drift 최신값과 history, agent
운영 로그, job output, target별 assignment 상태, pending approval, enrollment
token, policy assignment, runbook job 생성, audit event를 확인할 수 있습니다.
HTTP로 접속하면 화면 상단에 HTTP transport 경고도 표시합니다.

### 4. Enrollment token 만들기

Controller 컴퓨터에서 실행합니다.

```bash
TOKEN=$(sponzey enroll-token create \
  --data-dir .sponzey \
  --labels role=web,env=dev)
```

이 token은 agent 등록용입니다. admin token과 다릅니다.

바로 실행 가능한 agent 명령까지 출력하려면:

```bash
sponzey enroll-token create \
  --data-dir .sponzey \
  --labels role=web,env=dev \
  --controller-url http://127.0.0.1:7700 \
  --name web-01 \
  --print-init-command
```

### 5. Agent 초기화

Agent 컴퓨터에서 처음 한 번 실행합니다.

```bash
sponzey agent init \
  --data-dir .sponzey \
  --url http://127.0.0.1:7700 \
  --token "$TOKEN" \
  --name web-01 \
  --labels role=web,env=dev
```

### 6. Agent 시작

한 번만 확인하려면:

```bash
sponzey agent start \
  --data-dir .sponzey \
  --once
```

로컬 agent를 계속 켜두려면:

```bash
sponzey agent start \
  --data-dir .sponzey
```

Web Admin을 새로고침하면 agent 목록에 나타납니다.

`agent start`는 계속 살아있는 실행을 기본으로 합니다. Controller가 잠시
꺼져 있거나 네트워크가 끊겨도 기본적으로 계속 재시도합니다. 한 번만
확인하려면 `--once`를 쓰고, 반복 접속 실패 후 명시적으로 종료시키고 싶을
때만 `--max-reconnect-attempts <N>`을 사용합니다.

기본적으로 agent는 30초마다 product-safe 운영 로그 조각도 올립니다. 이는
agent 상태 이벤트이며, raw system log 파일을 자동 업로드하는 기능은
아닙니다. 주기는 `--log-upload-interval-seconds <SECONDS>`로 바꾸고,
업로드를 끄려면 `--disable-log-upload`을 사용합니다.

heartbeat, facts, metrics, 운영 로그는 각각 다른 주기를 가집니다.
heartbeat는 생존 신호일 뿐이며 task dispatch 주기를 제어하지 않습니다.
정적 inventory facts는 기본 300초마다 `--facts-interval-seconds` 기준으로
전송되고, 사용량 metrics는 기본 30초마다 `--metrics-interval-seconds`
기준으로 전송됩니다. task assignment는 이 telemetry 주기와 별개로
persistent session에서 push됩니다.

## Run은 어떻게 동작하나

Controller는 agent로 직접 접속하지 않습니다. Enrollment가 끝난 agent가
controller로 outbound persistent WebSocket session을 하나 열어 유지합니다.

Web Admin이나 `sponzey run`에서 명령을 실행하면 controller는 먼저 job과
controller가 서명한 task assignment를 DB에 저장합니다. 대상 agent가 현재
연결되어 있으면 controller는 이미 열린 session으로 task를 즉시 push합니다.
Run 경로는 다음 heartbeat를 기다리지 않습니다. Heartbeat는 생존 신호일
뿐입니다.

대상 agent가 offline이면 job은 queued 상태로 남습니다. agent가 다시
접속하고 인증되면 controller가 해당 agent의 pending assignment를 꺼내
새 session으로 다음 task를 push합니다.

Agent는 같은 session으로 `output_chunk` 여러 개와 `task_result` 하나를
controller에 돌려보냅니다. Web Admin은 표시 fallback으로 job detail API와
output API를 polling합니다. 이 방식으로 queued, delivered, running,
completed, no-output 상태를 보여주면서도 raw command output을 product log에
넣지 않습니다.

Agent key를 revoke하면 agent가 disabled 상태가 되고, active session이
있다면 `agent_revoked` reason으로 즉시 닫으며, 추가 task 전달을 막습니다.
Revoke는 job 중단 버튼이 아닙니다. 특정 job을 중단하려면
`POST /api/jobs/{job_id}/cancel`을 사용하거나, 해당 기능을 노출하는 UI/CLI
표면을 사용해야 합니다.

Cancel은 job과 assignment를 `canceled`로 기록합니다. Agent session이 active
상태이고 task가 이미 dispatch되었다면 controller는 기존 WebSocket session으로
`task_cancel`을 보냅니다. Agent는 현재 실행 중인 task id와 일치할 때 command
process를 kill하고 `task_result.status = "canceled"`를 보고합니다. Timeout은
cancel과 다릅니다. command timeout은 `task_result.status = "timed_out"`로
보고되고 controller는 job을 `expired`로 저장합니다.

## 대상 Preview와 Snapshot

Job을 만들기 전에 자동화 도구나 Web Admin은 `POST /api/selectors/preview`를
호출해서 대상 agent를 미리 확인할 수 있습니다. 요청은 string selector 또는
`matchLabels` 중 하나를 사용합니다.

```json
{ "matchLabels": { "role": "web", "env": "prod" } }
```

지원하는 string selector는 `agent:<name-or-id>`, `label:key=value`,
`key=value,key2=value2`입니다. Disabled 또는 revoked agent는 preview에는
나오지만 dispatch 대상에서 제외됩니다. Offline agent는 선택될 수 있고,
reconnect 전까지 assignment가 queued 상태로 남습니다.

Job이 생성되면 controller는 selector source와 target snapshot을 저장합니다.
이후 agent label이나 status가 바뀌어도 이미 생성된 job의 원래 대상 집합은
바뀌지 않습니다.

Multi-agent job은 preview 결과를 확인한 뒤 생성합니다. Controller는 해당
snapshot의 target마다 assignment를 하나씩 만듭니다. 선택적인 job `strategy`로
fanout 방식을 조절합니다.

```json
{
  "strategy": {
    "concurrency": 2,
    "maxFailures": 1
  }
}
```

`concurrency` 기본값은 `1`이며 순차 dispatch를 뜻합니다. `maxFailures`는
선택값입니다. 기준에 도달하면 아직 dispatch되지 않은 queued assignment를 더
실행하지 않고 `canceled`로 전환합니다. Job detail 응답은 저장된 strategy,
target별 `task_id`, `assignment_status`, `last_error`, 그리고 집계용
`assignment_summary` count object를 포함하므로 Web Admin과 자동화 도구가 연결
상태와 실행 상태를 구분할 수 있습니다.

## 위험 작업과 Approval

Sponzey는 위험 job을 만드는 것과 agent로 실제 dispatch하는 것을 분리합니다.

`uptime` 같은 안전한 단일 agent 확인 명령은 바로 queued 될 수 있습니다. shell
명령, `sudo`, `su`, reboot/shutdown, user/group 변경, package/service/file 변경,
알 수 없는 command, 여러 agent를 대상으로 하는 broad target은 approval request를
만듭니다. 이 job은 `pending_approval` 상태로 남고, approval이 승인되기 전까지
agent로 dispatch되지 않습니다.

`confirmed_high_risk`와 `--confirm-risk`는 과거 client 호환을 위한 확인 표시입니다.
approval을 대신하지 않습니다. Approval은 승인자, 사유, 상태, 만료 시각, audit
event를 남깁니다.

승인자는 입력창의 actor 값이 아니라 인증된 admin token에서 나온 actor로
결정됩니다. Approval request body에는 reason을 보낼 수 있고, UI가 보낸 actor
값은 audit이나 권한 판단에 사용하지 않습니다.

Approval API는 현재 사용할 수 있습니다.

```text
GET  /api/approvals?status=pending
POST /api/approvals/{approval_id}/approve
POST /api/approvals/{approval_id}/reject
POST /api/approvals/expire
```

Web Admin approval queue도 같은 API를 사용합니다. Approve/reject action은
decision reason만 보내며, controller는 인증된 admin token에서 approver를
결정합니다. 처리 후 approval, job, audit 화면을 다시 읽습니다.

## HTTPS 준비

제품, 고객, 운영, 공동 사용, 장시간 실행 환경에서는 이 준비가 필요합니다.
HTTP도 동작하지만 테스트 전용이며, Sponzey가 insecure HTTP 경고를 계속
출력합니다.

HTTPS를 제공하는 방법은 보통 두 가지입니다. 이 섹션은 두 번째 설정 흐름이
아니라 HTTPS 준비입니다.

HTTPS 준비가 끝나면 [하나의 설정 흐름](#하나의-설정-흐름)으로 돌아가서
로컬 값을 아래처럼 바꿉니다.

- `http://127.0.0.1:7700`을 HTTPS controller URL로 바꿉니다.
- 필요하면 `.sponzey`를 운영용 data directory로 바꿉니다.
- `agent start`는 운영용 data directory를 넣습니다.

HTTPS 인증서가 사설 CA 또는 self-signed라면 `agent init`에 아래 옵션도
추가합니다.

```bash
--tls-ca-cert /path/to/ca.pem
```

### Sponzey 내장 HTTPS

Controller 컴퓨터에 아래 파일을 준비합니다.

```text
/etc/sponzey/tls/fullchain.pem
/etc/sponzey/tls/privkey.pem
```

private key는 다른 사용자가 읽을 수 없어야 합니다.

```bash
sudo chmod 600 /etc/sponzey/tls/privkey.pem
```

Controller를 시작합니다.

```bash
sponzey controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir /var/lib/sponzey-fleet \
  --external-url https://fleet.example.com:7700 \
  --tls-cert /etc/sponzey/tls/fullchain.pem \
  --tls-key /etc/sponzey/tls/privkey.pem
```

### Reverse proxy HTTPS

Nginx, Caddy, load balancer 같은 도구가 HTTPS를 처리하는 방식입니다. 이 경우 Sponzey는 loopback에만 열어도 됩니다.

```bash
sponzey controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir /var/lib/sponzey-fleet \
  --external-url https://fleet.example.com
```

Proxy는 HTTPS 요청을 `127.0.0.1:7700`으로 전달하면 됩니다.

## SSH Tunnel 개발

SSH tunnel 개발도 설정 순서는 같습니다. 차이는 agent가 tunnel을 통해 local URL로 controller에 접속한다는 점뿐입니다.

Agent 컴퓨터에서 아래 명령을 계속 켜둡니다.

```bash
ssh -N -L 7700:127.0.0.1:7700 <user>@<controller-host>
```

그 다음 agent 컴퓨터에서는 아래 URL을 사용합니다.

```text
http://127.0.0.1:7700
```

Controller 컴퓨터의 LAN IP에 plain `http://`를 붙여도 동작합니다. 다만
Sponzey가 insecure HTTP 경고를 출력합니다.

## 로컬 스크립트

스크립트는 같은 단일 바이너리를 감싼 shortcut입니다.

```bash
./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir .sponzey --external-url http://127.0.0.1:7700
./scripts/run_agent.sh --data-dir .sponzey
```

중요:

- `run_controller.sh`는 `sponzey controller start`만 감쌉니다.
- `run_agent.sh`는 `sponzey agent start`만 감쌉니다.
- 스크립트가 `controller init`, `enroll-token create`, `agent init`을 대신 실행하지 않습니다.
- `scripts/run_agent.sh controller ...`처럼 실행하면 안 됩니다. agent 전용 스크립트입니다.

## Agent 삭제하기

먼저 agent를 중지합니다.

systemd service로 설치했다면:

```bash
sponzey agent uninstall-service --dry-run
sudo sponzey agent uninstall-service
```

그 다음 로컬 agent directory를 삭제합니다.

```bash
rm -rf .sponzey/agent
```

운영용 data directory라면:

```bash
sudo rm -rf /var/lib/sponzey-fleet/agent
```

Controller inventory와 audit 기록은 남습니다. 같은 host를 다시 쓰려면 새 enrollment token을 만들고 `sponzey agent init`을 다시 실행합니다.

## Controller 데이터 백업/복구

data directory를 삭제하거나, 다른 장비로 옮기거나, 위험한 운영 작업을 하기
전에는 controller를 백업하세요. 백업 중 SQLite DB에 쓰기가 들어가지 않도록
controller를 먼저 중지하는 것을 권장합니다.

```bash
sponzey controller backup \
  --data-dir .sponzey \
  --output ./sponzey-controller.backup.json
```

백업 archive에는 controller identity key와 SQLite data가 들어갑니다. 비밀값과
같은 수준으로 보관해야 합니다.

파일을 쓰지 않고 복구 가능 여부만 확인하려면:

```bash
sponzey controller restore \
  --data-dir ./restore-check \
  --input ./sponzey-controller.backup.json \
  --dry-run
```

빈 data directory로 복구하려면:

```bash
sponzey controller restore \
  --data-dir .sponzey-restored \
  --input ./sponzey-controller.backup.json
```

복구는 기존 controller directory를 자동으로 덮어쓰지 않습니다. 정말 교체해도
되는 대상인지 확인한 뒤에만 `--force`를 사용하세요.

전체 초기화는 data directory 전체를 삭제하면 됩니다.

```bash
rm -rf .sponzey
```

data directory 삭제는 reset입니다. Backup/restore는 controller identity,
inventory, jobs, audit events, telemetry, enrollment records를 보존하는
복구 경로입니다.

## 자주 나는 오류

### `controller is not initialized`

같은 data directory로 `controller init`을 한 번 실행해야 합니다.

### `unable to open database file`

대부분 controller data directory가 초기화되지 않은 경우입니다. 먼저 `sponzey controller init --data-dir ...`를 실행하세요.

### `agent is not enrolled`

`sponzey agent start ...` 전에 `sponzey agent init ...`을 먼저 실행해야 합니다.

### 실행 중인 job이 agent disconnect 후에도 running으로 남음

정상 동작입니다. Controller는 WebSocket이 끊겼다는 이유만으로 job을 failed로
바꾸지 않습니다. 최종 `task_result`, cancel, timeout, expiry 정책이 terminal
상태를 결정합니다. 실제 결과는 job output과 audit entries를 함께 확인해야 합니다.

### canceled, failed, expired가 다르게 보임

`canceled`는 operator cancel이 기록된 상태입니다. `failed`는 agent가 non-zero
또는 실패 결과를 보고한 상태입니다. `expired`는 timeout 또는 assignment expiry가
우선한 상태입니다. 세 상태는 의도적으로 분리됩니다.

### `WARNING: insecure HTTP controller URL enabled`

오류가 아닙니다. controller URL이 `http://`로 시작해서 controller-agent
통신이 암호화되지 않는다는 뜻입니다. HTTP는 테스트 전용입니다. 제품, 고객,
운영, 공동 사용, 장시간 실행 환경에서는 반드시 HTTPS를 사용해야 합니다.
HTTP transport는 기밀성이나 무결성 보장을 제공하지 않습니다.

### Web Admin에서 `{"error":"not_found"}`가 보임

API 주소를 연 것입니다. `/admin`으로 열어야 합니다.

### 어떤 token을 어디에 넣나?

- Web Admin UI: `sponzey controller init`이 출력한 admin token
- Agent init: `sponzey enroll-token create`가 출력한 enrollment token

## 개발 검증

전체 release gate는 [docs/release-gate.md](docs/release-gate.md)에 정리되어 있습니다.

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test --workspace @sponzey/fleet
npm test --workspace web-admin
npm run typecheck --workspace web-admin
npm run build --workspace web-admin
./scripts/smoke_mvp.sh
./scripts/smoke_immediate_dispatch.sh
./scripts/smoke_remote_tls_loopback.sh
```
