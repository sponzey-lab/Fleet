# Repository script source index

These scripts run local checks, package staging, release verification and
operator rehearsals. They do not own Fleet domain or controller state.

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `check_linux_glibc_baseline.sh` | Tooling | Checks Linux release binary baseline | Inspects binary metadata |
| `catalog_performance_gate.sh` | Tooling | Records and compares catalog benchmark timing evidence | Runs a deterministic Cargo test and writes/reads benchmark reports |
| `catalog_performance_gate.py` | Tooling | Implements catalog timing report validation and comparison | Runs benchmark subprocesses and reads/writes report JSON |
| `hardening_audit.sh` | Tooling | Audits source and package hardening invariants | Reads repository files |
| `manual_linux_nginx_runbook_smoke.sh` | Tooling | Rehearses Linux nginx runbook behavior | Manual privileged host actions |
| `manual_npm_registry_smoke.sh` | Tooling | Rehearses installed registry package behavior | Network/npm registry access |
| `manual_systemd_reboot_smoke.sh` | Tooling | Rehearses Fleet systemd persistence across reboot | Manual privileged systemd actions |
| `npm_demo_smoke.sh` | Tooling | Verifies packed wrapper demo flow | Builds and executes temporary package |
| `npm_local_pack_smoke.sh` | Tooling | Verifies packed wrapper launcher | Builds and executes temporary package |
| `npm_platform_local_install_smoke.sh` | Tooling | Verifies optional platform package installation | Builds, packages and executes temporary install |
| `npm_publish_current_platform.sh` | Tooling | Publishes/dry-runs the current platform package | npm registry credentials and publication |
| `npm_stage_current_platform.sh` | Tooling | Stages current native platform package | Builds and writes staging artifacts |
| `release_readiness_gate.sh` | Tooling | Runs local release readiness checks | Executes verification commands |
| `run_agent.sh` | Tooling | Starts local Fleet Agent development process | Executes Fleet binary |
| `run_controller.sh` | Tooling | Starts local Fleet Controller development process | Executes Fleet binary |
| `sign_release_sums.sh` | Tooling | Signs release checksums | Reads signing material and writes signature |
| `signature_verification_smoke.sh` | Tooling | Verifies release signature workflow | Builds temporary artifacts |
| `smoke_immediate_dispatch.sh` | Tooling | Verifies immediate task dispatch | Starts local Fleet processes |
| `smoke_mvp.sh` | Tooling | Verifies MVP controller-agent workflow | Starts local Fleet processes |
| `smoke_remote_tls_loopback.sh` | Tooling | Verifies TLS loopback workflow | Starts local Fleet processes |
| `storage_decision_gate.sh` | Tooling | Checks storage decision prerequisites | Reads repository state |
| `test_catalog_performance_gate.js` | Test | Verifies catalog benchmark report validation and threshold policy | Creates disposable JSON report fixtures and executes the gate script |
| `verify_release_signature.sh` | Tooling | Verifies signed release checksums | Reads public key and artifacts |
| `verify_standalone_artifacts.sh` | Tooling | Verifies standalone release artifacts | Reads release artifacts |
