#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

fail() {
  printf 'hardening audit failed: %s\n' "$1" >&2
  exit 1
}

if rg -n 'std::env::set_var|std::env::remove_var' crates >/tmp/sponzey-hardening-env-mutation.txt; then
  cat /tmp/sponzey-hardening-env-mutation.txt >&2
  fail "production code must not mutate process environment"
fi

if rg -n 'std::env::var|std::env::vars|std::env::var_os|std::env::vars_os' crates >/tmp/sponzey-hardening-env-read.txt; then
  cat /tmp/sponzey-hardening-env-read.txt >&2
  fail "production code must not read environment outside bootstrap settings"
fi

if rg -n 'tracing::(info|warn|error|debug|trace)!\([^;]*(stdout|stderr|output|private_key|token|secret)' crates >/tmp/sponzey-hardening-log-output.txt; then
  cat /tmp/sponzey-hardening-log-output.txt >&2
  fail "application logs must not include command output or secret-like fields"
fi

if rg -n '/api/.*/config|runtime_config|set_config|patch_config|std::env::set_var' crates >/tmp/sponzey-hardening-runtime-config.txt; then
  cat /tmp/sponzey-hardening-runtime-config.txt >&2
  fail "runtime configuration mutation endpoints are not allowed"
fi

rg -n 'parses_http_controller_url_for_remote_agent_with_warning_policy' crates/fleet-cli >/dev/null \
  || fail "remote HTTP agent warning-policy test is missing"

rg -n 'controller_allows_remote_http_external_url' crates/fleet-controller >/dev/null \
  || fail "remote HTTP controller warning-policy test is missing"

rg -n 'controller_marks_plain_http_listener_without_external_url' crates/fleet-controller >/dev/null \
  || fail "plain HTTP listener warning-policy test is missing"

rg -n 'insecure_http_transport_start_is_audited' crates/fleet-controller >/dev/null \
  || fail "insecure HTTP security audit test is missing"

rg -n 'rejects_unsigned_envelope|invalid_signature_is_rejected|expired_task_is_rejected|replayed_nonce_is_rejected|target_mismatch_is_rejected' crates >/dev/null \
  || fail "signed task envelope rejection tests are missing"

rg -n 'high_risk_command_without_confirmation_is_rejected|high_risk_run_requires_confirmation|command_job_requires_high_risk_confirmation|high_risk_runbook_without_confirmation_is_rejected' crates >/dev/null \
  || fail "high-risk confirmation tests are missing"

rg -n 'command_output_is_redacted_before_rendering|redacts_token_like_values|redacts_multiple_secret_markers' crates >/dev/null \
  || fail "redaction tests are missing"

rg -n 'enrollment_token_create_is_audited_without_raw_token|agent_security_event_is_audited|auth_failure_writes_security_audit' crates >/dev/null \
  || fail "security/audit coverage tests are missing"

printf 'hardening audit ok\n'
