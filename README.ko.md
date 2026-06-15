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

| 데이터     | 뜻                                                                 |
| ------- | ----------------------------------------------------------------- |
| Facts   | 거의 변하지 않는 인벤토리입니다. OS, 아키텍처, hostname, CPU 코어 수, 메모리 총량/모듈 수, 디스크 총 용량, 네트워크 인터페이스 등을 담습니다. |
| Metrics | 시간에 따라 변하는 사용량입니다. CPU 사용률, 메모리 사용량, 디스크 사용량, 프로세스 수, 실패한 서비스 수 등을 담습니다. |

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
합니다. 상세 API 계약은 [docs/api.md](docs/api.md)에 유지합니다.

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

전체 초기화는 data directory 전체를 삭제하면 됩니다.

```bash
rm -rf .sponzey
```

## 자주 나는 오류

### `controller is not initialized`

같은 data directory로 `controller init`을 한 번 실행해야 합니다.

### `unable to open database file`

대부분 controller data directory가 초기화되지 않은 경우입니다. 먼저 `sponzey controller init --data-dir ...`를 실행하세요.

### `agent is not enrolled`

`sponzey agent start ...` 전에 `sponzey agent init ...`을 먼저 실행해야 합니다.

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

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets
npm test --workspace @sponzey/fleet
npm test --workspace web-admin
npm run build --workspace web-admin
./scripts/smoke_mvp.sh
```
