# Task 013: Facts/Metrics/Logs Schema와 Retention

상태: Completed
우선순위: P1
연결 계획: `.tasks/plan.md` Phase 010, Phase 011
의존성: Task 002
결과물: telemetry 의미 경계와 retention 기준

## 목표

Facts, Metrics, Logs의 의미를 코드/API/UI/문서에서 일관되게 만든다. Facts는 static inventory, Metrics는 usage telemetry, Logs는 operational event/log stream으로 분리한다.

## 기능 묶음

1. facts inventory schema 안정화
2. metrics/logs time-series contract 정리
3. retention policy 기초

## 구현 체크리스트

Facts:

- [x] 현재 facts payload를 조사한다.
- [x] memory usage 값이 facts에 남아 있으면 metrics로 이동한다.
- [x] disk usage 값이 facts에 남아 있으면 metrics로 이동한다.
- [x] disk inventory에 disk/partition/mount/filesystem/total size를 포함한다.
- [x] memory inventory에 total과 module count 가능 여부를 반영한다.
- [x] network inventory에 interface identity를 정리한다.
- [x] agent system time과 stored at을 보존한다.

Metrics:

- [x] 현재 metrics payload를 조사한다.
- [x] cpu usage를 metrics로 유지한다.
- [x] memory used/available/percent를 metrics로 유지한다.
- [x] disk used/available/percent를 metrics로 유지한다.
- [x] metrics paging contract를 확인한다.
- [x] metrics chart range와 API query가 맞는지 확인한다.

Logs:

- [x] agent operational log와 job output을 구분한다.
- [x] command stdout/stderr 원문이 product log에 남지 않게 확인한다.
- [x] log upload interval이 heartbeat와 독립적인지 확인한다.
- [x] logs paging contract를 정리한다.
- [x] journald/file tail adapter는 scope를 분리한다.

Retention:

- [x] facts retention 정책을 정한다.
- [x] metrics retention 정책을 정한다.
- [x] logs retention 정책을 정한다.
- [x] job output retention 정책을 정한다.
- [x] audit은 일반 retention으로 삭제하지 않는다.
- [x] retention cleanup fake clock/cutoff test를 준비한다.

## 테스트

- [x] facts schema serialization test
- [x] metrics schema serialization test
- [x] facts does not include usage telemetry test
- [x] metrics includes usage telemetry test
- [x] logs and job output separation test
- [x] paging contract test
- [x] retention fake clock/cutoff test
- [x] audit not deleted by retention test

## 검증 명령

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## 문서 업데이트

- [x] docs/api.md에 facts/metrics/logs 의미를 분리해서 적는다.
- [x] README 또는 docs에 Agent time과 Stored at 차이를 설명한다.
- [x] Web Admin labels/help text를 현재 의미와 맞춘다.

## 완료 기준

- [x] facts에는 static inventory만 남는다.
- [x] metrics에는 usage telemetry가 저장된다.
- [x] logs와 job output이 섞이지 않는다.
- [x] retention 정책이 audit를 삭제하지 않는다.

## 구현 메모

- Facts는 static inventory로 정리했다. 현재 CPU logical count, memory total/module count, Linux disk/partition inventory, mount layout, root filesystem total size, network interface identity를 담고, memory/disk usage 값은 담지 않는다.
- Metrics는 usage telemetry로 유지한다. CPU usage, memory used/available/percent, disk used/available/percent, process/service usage성 값을 담는다.
- Agent operational log는 `agent_log_chunks`에 저장하고 `GET /api/agents/{agent_id}/logs?limit=...&before=...` cursor paging API로 조회한다.
- Job stdout/stderr는 계속 `job_output_chunks`와 `GET /api/jobs/{job_id}/output`에만 속한다. Product application log와 agent operational log에 command output 원문을 섞지 않는다.
- Explicit retention cleanup은 `job_output_chunks`, `facts_snapshots`, `metrics_snapshots`, `agent_log_chunks`를 대상으로 하며 `audit_events`는 삭제하지 않는다.
- Background retention worker와 remote raw file/journald streaming은 아직 후속 phase 범위로 남긴다.