# Sponzey Fleet

[English README](README.md)

Sponzey Fleet는 여러 컴퓨터를 한곳에서 관리하는 도구입니다. 모든 컴퓨터에 같은
`fleet` 프로그램을 설치하고, 실행할 때 역할을 선택합니다.

- **Controller**는 관리 본부입니다. 데이터를 저장하고, Web Admin 화면을 제공하고,
  작업에 서명하고, Agent의 연결을 받습니다.
- **Agent**는 관리할 컴퓨터마다 실행합니다. Controller에 접속해서 컴퓨터 정보와
  사용량을 보내고, Controller가 서명한 작업을 실행합니다.

Controller 하나에 Agent 여러 대를 연결할 수 있습니다. Controller가 Agent로 접속하는
방식이 아니라, 각 Agent가 Controller로 연결하는 방식입니다.

> npm 패키지 이름은 `@sponzey/fleet`이지만 현재 실행 명령은 `fleet`입니다.

## 준비물

처음 시작할 때는 다음이 있으면 됩니다.

- macOS 또는 Linux 컴퓨터
- Node.js와 npm
- 터미널
- 웹 브라우저

Node.js와 npm이 설치되어 있는지 확인합니다.

```bash
node --version
npm --version
```

둘 중 하나라도 명령을 찾을 수 없다고 나오면 Node.js LTS 버전을 먼저 설치하세요.

## 설치하기

어느 터미널에서든 `fleet` 명령을 사용할 수 있도록 전역 설치합니다.

```bash
npm install -g @sponzey/fleet
fleet --version
```

`npm install @sponzey/fleet`처럼 프로젝트 안에만 설치했다면 `fleet` 대신
`npx fleet`로 실행합니다.

설치 후 `fleet: command not found`가 나온다면 npm 명령 설치 경로를 확인합니다.

```bash
echo "$(npm prefix -g)/bin"
```

출력된 경로를 shell의 `PATH`에 추가하고 새 터미널을 여세요.

이 저장소에서 직접 빌드하려면 다음 명령을 사용합니다.

```bash
cargo build -p fleet-cli
./target/debug/fleet --version
```

소스 빌드를 사용할 때는 아래 예시의 `fleet`를 `./target/debug/fleet`로 바꾸면
됩니다.

## 명령 하나로 데모 실행하기

실제 설정을 시작하기 전에 데모부터 실행해 볼 수 있습니다.

```bash
fleet demo
```

임시 Controller와 Agent를 만들고, 작은 작업을 실행한 뒤 Web Admin 주소를 보여줍니다.
데모가 끝나면 임시 데이터는 자동으로 정리됩니다.

## 꼭 알아야 할 값 세 가지

설정하는 동안 다음 값을 만나게 됩니다.

| 값 | 무엇인가요? | 어디에 사용하나요? |
| --- | --- | --- |
| Admin token | 관리자가 사용하는 비밀번호 | Web Admin에 입력하거나 보호된 CLI/API 명령에 사용합니다 |
| Enrollment token | Agent를 처음 등록할 때 쓰는 짧은 유효기간의 일회용 코드 | `fleet agent init`에만 사용합니다 |
| Data directory | 키, 설정, Controller 또는 Agent 데이터가 들어 있는 폴더 | 해당 역할을 다시 시작할 때 항상 같은 폴더를 사용합니다 |

Admin token과 Enrollment token은 서로 다른 값입니다. 바꿔서 사용할 수 없습니다.

## 초보자 설정: 컴퓨터 한 대에서 Controller와 Agent 실행하기

처음 배울 때 가장 안전한 방법입니다. 같은 컴퓨터에서 터미널 두 개를 사용합니다.

### 1단계: Controller 초기화

첫 번째 터미널을 열고 실행합니다.

```bash
mkdir -p fleet-controller
fleet controller init --data-dir ./fleet-controller
```

명령을 실행하면 `admin token`이 한 번 표시됩니다. 비밀번호 관리자나 안전한 임시
공간에 복사해 두세요. Controller에는 token 원문이 아니라 hash만 저장되므로 같은
token을 다시 보여주지 않습니다.

Controller 초기화는 보통 처음 한 번만 합니다.

### 2단계: Controller 시작

같은 터미널에서 실행합니다.

```bash
fleet controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir ./fleet-controller \
  --external-url http://127.0.0.1:7700
```

이 터미널은 닫지 마세요. `Ctrl+C`를 누르거나 터미널을 닫으면 Controller도
종료됩니다.

여기서는 한 컴퓨터에서 연습하므로 `http://`를 사용합니다. HTTP는 암호화되지 않기
때문에 Sponzey가 경고를 표시합니다.

### 3단계: Web Admin 열기

브라우저에서 다음 주소를 엽니다.

```text
http://127.0.0.1:7700/admin
```

1단계에서 저장한 admin token을 입력합니다. 아직 Agent가 없어도 Web Admin 화면은
열려야 합니다.

### 4단계: Agent 등록 token 만들기

두 번째 터미널을 엽니다. Controller는 계속 실행 중이어야 합니다.

```bash
fleet enroll-token create \
  --data-dir ./fleet-controller \
  --labels role=test,env=local
```

출력된 token을 복사하세요. 이 값은 Enrollment token이며 admin token이 아닙니다.
Web Admin에서도 Enrollment token을 만들 수 있습니다.

### 5단계: Agent 초기화

아래 명령의 `PASTE_ENROLLMENT_TOKEN_HERE`를 4단계에서 받은 token으로 바꾼 다음
실행합니다.

```bash
mkdir -p fleet-agent
fleet agent init \
  --data-dir ./fleet-agent \
  --url http://127.0.0.1:7700 \
  --token PASTE_ENROLLMENT_TOKEN_HERE \
  --name my-first-agent \
  --labels role=test,env=local
```

Agent 초기화 과정에서 Agent identity를 만들고 Controller identity를 고정합니다.
하나의 Agent data directory에서는 보통 처음 한 번만 실행합니다.

### 6단계: Agent 시작

두 번째 터미널에서 실행합니다.

```bash
fleet agent start --data-dir ./fleet-agent
```

이 터미널도 계속 열어 두세요. Web Admin을 새로 고치면 Agent 목록에
`my-first-agent`가 보여야 합니다.

연결을 한 번만 확인하고 종료하려면 다음처럼 실행합니다.

```bash
fleet agent start --data-dir ./fleet-agent --once
```

일반 실행에서는 네트워크나 Controller가 잠시 끊겨도 Agent가 계속 재접속을
시도합니다.

## 실제 설정: Controller와 Agent를 서로 다른 컴퓨터에서 실행하기

설정 순서는 같습니다. 다만 Agent에서 Controller 컴퓨터의 실제 IP 주소나 DNS 이름을
사용해야 합니다.

아래에서는 Controller 주소가 `192.168.0.10`이라고 가정합니다. 이 값을 실제 주소로
바꿔서 사용하세요.

### 1단계: Controller 컴퓨터의 주소 확인

Linux에서는 다음 명령을 사용할 수 있습니다.

```bash
hostname -I
```

macOS Wi-Fi에서는 다음 명령을 사용할 수 있습니다.

```bash
ipconfig getifaddr en0
```

공유기나 클라우드 서버 관리 화면에서도 주소를 확인할 수 있습니다.

### 2단계: Controller 초기화와 시작

Controller 컴퓨터에서 실행합니다.

```bash
mkdir -p fleet-controller
fleet controller init --data-dir ./fleet-controller
```

출력된 admin token을 안전하게 보관한 다음 Controller를 시작합니다.

```bash
fleet controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir ./fleet-controller \
  --external-url http://192.168.0.10:7700
```

여기서 두 주소는 의미가 다릅니다.

- `--host 0.0.0.0`은 모든 network interface에서 연결을 받겠다는 bind 값입니다.
  Agent가 접속할 수 있는 주소가 아닙니다.
- `--external-url http://192.168.0.10:7700`은 Agent와 관리자가 실제로 사용하는
  주소입니다.

`--external-url`이나 `agent init --url`에는 절대로 `0.0.0.0`을 넣지 마세요.

선택한 port가 firewall에서 허용되어야 합니다. 다른 컴퓨터에서
`http://192.168.0.10:7700/admin`이 열리지 않는다면 Controller 실행 여부, IP 주소,
공유기/network 정책, 운영체제와 cloud firewall을 먼저 확인하세요.

### 3단계: Controller에서 Enrollment token 만들기

Controller 컴퓨터에서 실행합니다.

```bash
fleet enroll-token create \
  --data-dir ./fleet-controller \
  --labels role=web,env=test
```

출력된 token을 Agent 컴퓨터로 안전하게 전달합니다. Enrollment token은 유효기간이
짧으며, 채팅방·업무 티켓·소스 코드에 올리면 안 됩니다.

### 4단계: Agent 컴퓨터 설치와 초기화

Agent 컴퓨터에서 실행합니다.

```bash
npm install -g @sponzey/fleet
mkdir -p fleet-agent
fleet agent init \
  --data-dir ./fleet-agent \
  --url http://192.168.0.10:7700 \
  --token PASTE_ENROLLMENT_TOKEN_HERE \
  --name web-01 \
  --labels role=web,env=test
```

이어서 Agent를 시작합니다.

```bash
fleet agent start --data-dir ./fleet-agent
```

`http://192.168.0.10:7700/admin`을 열어 `web-01`이 online으로 표시되는지 확인합니다.

> `127.0.0.1`은 항상 “지금 이 컴퓨터”를 뜻합니다. Agent가 다른 컴퓨터에 있다면
> SSH tunnel을 사용하지 않는 이상 `127.0.0.1`을 Controller 주소로 쓰면 안 됩니다.

## 실제 운영이나 장시간 실행에는 HTTPS 사용하기

HTTP는 admin token, Agent 등록 과정, 작업, Agent 데이터를 암호화하지 않습니다.
HTTP는 로컬 연습, 사설 lab, 짧은 테스트에만 사용하세요. 운영 환경, 고객 환경, 여러
사람이 사용하는 환경, 인터넷에 연결된 환경, 장시간 실행하는 환경에서는 HTTPS가
필수입니다.

### 방법 A: Sponzey 내장 HTTPS

Controller에 인증서 체인과 private key를 준비한 다음 실행합니다.

```bash
fleet controller start \
  --host 0.0.0.0 \
  --port 7700 \
  --data-dir /var/lib/fleet \
  --external-url https://fleet.example.com:7700 \
  --tls-cert /etc/fleet/tls/fullchain.pem \
  --tls-key /etc/fleet/tls/privkey.pem
```

TLS private key는 Controller를 실행하는 계정만 읽을 수 있어야 합니다.

Agent도 같은 HTTPS 주소로 초기화합니다.

```bash
fleet agent init \
  --data-dir /var/lib/fleet \
  --url https://fleet.example.com:7700 \
  --token PASTE_ENROLLMENT_TOKEN_HERE \
  --name web-01 \
  --labels role=web,env=prod
```

사설 CA나 self-signed 인증서를 사용한다면 Agent 초기화 명령에 다음 옵션을
추가합니다.

```text
--tls-ca-cert /path/to/ca.pem
```

### 방법 B: HTTPS reverse proxy

Nginx, Caddy, cloud load balancer 같은 reverse proxy가 HTTPS를 처리하게 할 수도
있습니다. 이 경우 Sponzey는 Controller의 loopback에서만 실행합니다.

```bash
fleet controller start \
  --host 127.0.0.1 \
  --port 7700 \
  --data-dir /var/lib/fleet \
  --external-url https://fleet.example.com
```

Proxy가 HTTPS 요청과 WebSocket 연결을 모두 `127.0.0.1:7700`으로 전달하도록
설정해야 합니다.

## Web Admin에서 할 수 있는 일

Agent가 연결된 뒤 Web Admin에서 다음 작업을 할 수 있습니다.

- Agent online/offline 상태와 inventory 확인
- facts, metrics, drift history, 안전하게 정리된 Agent log 확인
- command와 runbook job 생성
- 여러 Agent 작업을 만들기 전에 selector 대상 미리 보기
- Agent별 assignment 상태와 job output 확인
- approval 요청 생성, 승인, 거절
- policy 저장·배정과 drift check 일정 설정
- 공개 catalog source, 검증된 revision, runbook/policy 메타데이터 확인
- remediation 진행 상태와 audit event 확인

Job은 Agent로 보내기 전에 먼저 저장됩니다. Agent가 offline이면 assignment가 queued
상태로 남고, Agent가 다시 연결된 뒤 전달될 수 있습니다.

## Policy와 remediation을 쉽게 이해하기

**Runbook**은 “지금 이 단계들을 실행하라”는 문서입니다. **Policy**는 “이 상태가 계속
유지되어야 한다”는 문서입니다.

예를 들어 `role=web` label이 있는 모든 Agent에서 nginx가 실행 중이어야 한다는 policy를
만들 수 있습니다.

Remediation은 다음 순서로 진행됩니다.

1. 서명된 drift check가 컴퓨터 상태와 policy가 다르다고 보고합니다.
2. Controller가 remediation 제안을 만듭니다.
3. 운영자가 내용을 확인하고 승인합니다.
4. Controller가 runbook job을 만들고 서명합니다.
5. Agent가 job을 실행하고 결과를 보고합니다.
6. Controller가 verification check를 실행합니다.
7. 새로 확인된 compliant 증거가 remediation과 원본 drift를 resolved 처리합니다.

Remediation은 approval을 우회하지 않습니다. 예전 수동 `running`, `result`, `verify`
API/CLI 명령은 deprecated 상태이며 `409`를 반환합니다. 인증된 Agent event와 저장된
verification evidence가 실제 상태의 기준입니다.

전체 문법과 고급 예시는 [Runbook 문서](docs/runbooks.md),
[Policy 문서](docs/policy.md), [API 계약](docs/api.md)을 참고하세요.

## 공개 runbook·policy catalog 추가하기

Catalog는 검증 가능한 runbook과 policy 문서를 담은 공개 HTTPS Git 저장소입니다. Source를
등록해도 바로 내려받지 않고, sync가 끝나도 바로 활성화되지 않으며, 활성화해도 Agent에서
실행되지는 않습니다. 검토할 시간을 확보하기 위해 세 단계를 분리했습니다.
CI workflow처럼 catalog와 관계없는 YAML 파일은 건너뜁니다. 다만 Fleet `Runbook` 또는
`Policy`라고 선언한 파일은 sync가 성공하려면 반드시 올바른 문서여야 합니다.

Web Admin에서 **Runbooks** 또는 **Policies** 메뉴를 열고 Catalog 패널에서 다음 순서로
진행합니다.

1. 짧은 source ID, 공개 `https://` Git URL, 따라갈 branch 또는 tag를 입력한 뒤
   **Register source**를 누릅니다.
2. `sync-2026-08-31-01`처럼 새 operation ID를 입력하고 **Start sync**를 누릅니다.
   요청은 먼저 저장되고 Controller worker가 나중에 처리하므로, 완료된 revision 상태는
   Catalog 새로 고침으로 확인합니다.
3. 준비된(ready) revision을 선택하고 전체 commit ID를 **Ready commit**에 붙여 넣은 뒤,
   검토를 마쳤을 때만 **Activate ready revision**을 누릅니다.

세 열은 source, revision, document의 메타데이터만 보여 줍니다. 문서 본문을 목록에
노출하지 않으며, sync 성공을 활성화로 간주하지도 않습니다.

`fleet login` 이후에는 같은 작업을 CLI로도 할 수 있습니다.

```bash
fleet catalog register public-operations https://example.com/operations.git main
fleet catalog sync public-operations sync-2026-08-31-01
fleet catalog list
fleet catalog activate public-operations READY_REVISION의_전체_COMMIT_ID
```

Catalog 등록·sync·활성화는 owner 또는 administrator만 할 수 있습니다. 공개 HTTPS
저장소만 사용하며 private 저장소 credential은 catalog 설정으로 지원하지 않습니다.

## 선택 사항: 운영자 CLI 로그인

처음에는 Web Admin을 사용하는 것이 가장 쉽습니다. CLI를 사용하고 싶다면 Controller
주소와 admin token을 로컬 profile에 저장할 수 있습니다.

```bash
fleet login \
  --controller-url https://fleet.example.com \
  --admin-token PASTE_ADMIN_TOKEN_HERE
```

이후 다음 명령이 해당 profile을 사용합니다.

```bash
fleet agents remote-list
fleet jobs list
fleet approvals list
fleet remediations list
fleet audit export --category security --limit 100
```

Profile에는 운영자 credential이 들어 있습니다. 다른 사용자에게 복사하거나 source
control에 commit하지 마세요.

## Linux에서 계속 실행하기

systemd를 사용하는 Linux에서는 먼저 service가 사용할 data directory로 Controller나
Agent를 초기화합니다. 설치 전에 생성될 unit을 확인하세요.

```bash
fleet controller install-service \
  --data-dir /var/lib/fleet \
  --dry-run

fleet agent install-service \
  --data-dir /var/lib/fleet \
  --dry-run
```

Service 설치와 삭제에는 Linux와 root 권한이 필요합니다.

```bash
sudo fleet agent install-service --data-dir /var/lib/fleet
sudo fleet agent start-service
sudo fleet agent status-service
sudo fleet agent logs-service
```

Controller service 명령도 같은 형태입니다. 운영에 사용하기 전 dry-run 출력과
HTTPS/reverse proxy 설정이 맞는지 반드시 확인하세요.

## Controller 백업하기

업그레이드, migration, 다른 컴퓨터로 이동하기 전에는 Controller를 백업하세요.
SQLite에 쓰는 중이 아니도록 Controller를 먼저 종료하는 것이 안전합니다.

```bash
fleet controller backup \
  --data-dir ./fleet-controller \
  --output ./fleet-controller.backup.json
```

백업 파일에는 Controller key와 운영 데이터가 포함됩니다. 비밀 정보처럼 보관하세요.

파일을 쓰지 않고 백업을 검사할 수 있습니다.

```bash
fleet controller restore \
  --data-dir ./restore-check \
  --input ./fleet-controller.backup.json \
  --dry-run
```

빈 data directory에 복구하려면 다음처럼 실행합니다.

```bash
fleet controller restore \
  --data-dir ./fleet-controller-restored \
  --input ./fleet-controller.backup.json
```

## 자주 생기는 문제

### `fleet: command not found`

전역 설치 후 새 터미널을 여세요. 그래도 안 되면 `$(npm prefix -g)/bin`을 확인하고
해당 경로를 `PATH`에 추가합니다.

### `controller is not initialized`

`fleet controller init`을 한 번 실행합니다. `controller init`과 `controller start`의
`--data-dir`이 정확히 같은지 확인하세요.

### `agent is not enrolled`

`fleet agent init`을 한 번 실행합니다. `agent init`과 `agent start`의 Agent
`--data-dir`이 정확히 같은지 확인하세요.

### Agent가 Controller에 연결되지 않음

다음 순서로 확인하세요.

1. Controller 터미널이나 service가 계속 실행 중인가요?
2. Agent가 자신의 `127.0.0.1`이 아니라 Controller의 IP 또는 DNS 이름을 사용하나요?
3. 운영체제와 cloud firewall에서 `7700` port가 열려 있나요?
4. Controller가 TLS를 사용한다면 URL도 `https://`로 시작하나요?
5. 사설 CA라면 Agent 초기화 때 `--tls-ca-cert`를 지정했나요?

### Web Admin에 Agent가 보이지 않음

Agent 터미널에서 등록, identity, 연결 오류를 확인하세요. Enrollment token은 보통 한
번만 사용할 수 있으므로 이미 사용한 token을 반복해서 쓰지 말고 새 token을 만드세요.

### `WARNING: insecure HTTP controller URL enabled`

오류가 아니라 경고입니다. 통신이 암호화되지 않았다는 뜻입니다. 로컬 또는 짧은
테스트가 아니라면 HTTPS를 사용하세요.

### Web Admin에 `{"error":"not_found"}`가 표시됨

`/admin` 주소를 여세요. 예: `http://127.0.0.1:7700/admin`

### Job이 계속 queued 상태임

대상 Agent가 offline이거나 앞선 assignment가 끝나기를 기다리고 있을 수 있습니다.
Agent를 시작하고 Job과 Audit 화면을 확인하세요. queued 작업을 완료된 것으로 처리하지
않습니다.

## 보안 주의사항

- Admin token, Enrollment token, private key, backup 파일을 commit하지 마세요.
- 실제 환경에서는 HTTPS를 사용하세요.
- Controller, Agent, TLS private key 파일은 필요한 service 계정만 읽을 수 있게 하세요.
- 권한이 필요한 작업을 실행하기 전에 target preview와 approval 내용을 확인하세요.
- 업그레이드나 위험한 유지보수 전에는 Controller를 백업하세요.

자세한 내용은 [보안 문서](docs/security.md), [저장소 문서](docs/storage.md),
[기능 매트릭스](docs/feature-matrix.md)를 참고하세요.

## API와 개발 문서

Controller는 다음 주소를 제공합니다.

```text
/admin         Web Admin
/openapi.json  OpenAPI 3.1 JSON
/swagger-ui    API 테스트 화면
```

HTTP로 연 Swagger UI에는 실제 admin token을 입력하지 마세요.

개발자가 주로 사용하는 검증 명령은 다음과 같습니다.

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test --workspace @sponzey/fleet
npm test --workspace web-admin
npm run typecheck --workspace web-admin
npm run build --workspace web-admin
```

전체 release 절차는 [release gate](docs/release-gate.md)를 참고하세요.

## 라이선스

Sponzey Fleet는 GNU Affero General Public License version 3 only
(`AGPL-3.0-only`)를 사용합니다. 별도 표시가 없는 한 Rust workspace, Web Admin, npm
wrapper, 배포 binary에 같은 라이선스가 적용됩니다. [LICENSE](LICENSE)와
[라이선스 설명](docs/license.md)을 참고하세요.
