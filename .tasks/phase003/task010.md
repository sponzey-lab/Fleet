# Task 010 - Backpressure와 Output 안정성

상태: `[ ] 대기` `[ ] 진행 중` `[x] 완료`

## 목표

persistent session에서 output과 outbound task가 Controller 또는 Agent를 압박하지 않도록 안정성 경계를 구현한다.

상시 연결은 반응성을 높이지만, 동시에 긴 command output, 느린 DB write, 느린 network writer, queue overflow 같은 문제가 더 중요해진다.

## 기능 범위

### 1. Per-session outbound queue 제한

- [x] session당 outbound queue capacity를 명시한다.
- [x] queue full 시 정책을 정한다.
- [x] queue depth를 FieldDebug 또는 session summary에 노출할 수 있게 한다.

정책 후보:

- task_assignment queue full: dispatch failure로 기록하고 queued 유지
- heartbeat/log queue full: drop 가능 여부 검토
- output queue full: task failure 또는 session failure로 전환

원칙:

- 무제한 메모리 증가 금지
- task 유실 금지
- Product log에 raw output 기록 금지

### 2. Output chunk 안정성

- [x] max output bytes 정책을 persistent session에서도 유지한다.
- [x] output chunk size와 sequence ordering을 재검증한다.
- [x] duplicate output chunk 처리 정책을 구현한다.

현재 DB unique:

```text
UNIQUE(job_id, agent_id, stream, chunk_index)
```

권장:

- 같은 key + 같은 body: idempotent duplicate
- 같은 key + 다른 body: security/audit 또는 protocol error
- insert duplicate 때문에 session 전체가 불필요하게 죽지 않게 한다.

### 3. Long-running command와 disconnect 처리

- [x] command timeout을 강제한다.
- [x] running 중 Controller connection drop 시 Agent task worker 정책을 정한다.
- [x] running 중 Agent disconnect 시 Controller job 상태 정책을 정한다.

초기 정책:

- Agent local command는 timeout까지 실행될 수 있다.
- connection이 끊기면 output/result 전송 실패로 task failure 또는 reconnect 후 재전송 불가 상태가 될 수 있다.
- Controller는 task_result 없는 running job을 즉시 failed로 만들지 않고 expiry/reconciler 정책을 따른다.

## 테스트와 검증

필수:

- [x] output limit exceeded test
- [x] outbound queue full test
- [x] duplicate output chunk idempotency test
- [x] duplicate output chunk body mismatch test
- [x] slow store/write failure test
- [x] running 중 disconnect 시 job 상태 정책 test
- [x] command timeout test
- [x] `cargo test -p fleet-runner output`
- [x] `cargo test -p fleet-store job_output_chunks`
- [x] `cargo test -p fleet-controller websocket`
- [x] `cargo fmt --all --check`
- [x] `git diff --check`

## 완료 기준

- [x] Agent 하나의 과도한 output이 Controller 전체를 막지 않는다.
- [x] outbound queue는 bounded다.
- [x] output은 job output storage에만 저장되고 Product application log에 원문이 남지 않는다.
- [x] duplicate output과 disconnect 정책이 테스트로 고정된다.
- [x] long-running command timeout이 유지된다.

## 비범위

- [x] full streaming admin WebSocket 구현하지 않음
- [x] HA/outbox 대규모 재설계하지 않음
- [x] command cancellation protocol 구현하지 않음