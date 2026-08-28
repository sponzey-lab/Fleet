#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

fail() {
  printf 'hardening audit failed: %s\n' "$1" >&2
  exit 1
}

if rg -n 'std::env::set_var|std::env::remove_var' crates >/tmp/fleet-hardening-env-mutation.txt; then
  cat /tmp/fleet-hardening-env-mutation.txt >&2
  fail "production code must not mutate process environment"
fi

if rg -n 'std::env::var|std::env::vars|std::env::var_os|std::env::vars_os' crates \
  | grep -v 'FLEET_TEST_POSTGRES_URL' >/tmp/fleet-hardening-env-read.txt; then
  cat /tmp/fleet-hardening-env-read.txt >&2
  fail "production code must not read environment outside bootstrap settings"
fi

if rg -n 'tracing::(info|warn|error|debug|trace)!\([^;]*(stdout|stderr|output|private_key|token|secret)' crates >/tmp/fleet-hardening-log-output.txt; then
  cat /tmp/fleet-hardening-log-output.txt >&2
  fail "application logs must not include command output or secret-like fields"
fi

if rg -n '/api/.*/config|runtime_config|set_config|patch_config|std::env::set_var' crates >/tmp/fleet-hardening-runtime-config.txt; then
  cat /tmp/fleet-hardening-runtime-config.txt >&2
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

rg -n 'secret_ref_accepts_supported_reference_format|static_secret_provider_resolves_without_displaying_raw_secret|static_secret_provider_errors_are_typed_and_redacted' crates >/dev/null \
  || fail "secret provider boundary/redaction tests are missing"

rg -n 'secret_provider_settings_default_to_disabled_mode|secret_provider_settings_reject_unsupported_kind_without_secret_leak|secret_provider_settings_reject_inline_raw_secret_candidate' crates >/dev/null \
  || fail "secret provider bootstrap settings tests are missing"

rg -n 'disabled_secret_provider_denies_without_reference_leak|controller_secret_provider_factory_builds_disabled_from_default_settings|controller_secret_provider_factory_builds_static_test_from_explicit_source|controller_secret_provider_factory_rejects_static_test_without_source_redacted' crates >/dev/null \
  || fail "secret provider construction tests are missing"

rg -n 'agent_runbook_secret_handoff_disabled_provider_rejects_without_ref_leak|agent_runbook_secret_handoff_static_provider_renders_without_artifact_body|apply_validation_does_not_resolve_secret_backed_templates' crates >/dev/null \
  || fail "agent secret resolver handoff tests are missing"

rg -n 'trust_settings_keep_tls_and_signing_identities_distinct|trust_settings_reject_tls_private_key_reused_as_signing_private_key|trust_settings_do_not_derive_controller_signing_from_tls_fingerprint|controller_trust_settings_reject_tls_key_reused_as_signing_key_without_path_leak' crates >/dev/null \
  || fail "TLS/signing identity separation tests are missing"

rg -n 'parses_controller_start_agent_client_ca_cert|controller_rejects_agent_client_ca_cert_until_mtls_enforcement_exists|agent client certificate mTLS enforcement is not implemented' crates docs >/dev/null \
  || fail "agent client certificate mTLS bootstrap guard coverage is missing"

rg -n 'agent_certificate_lifecycle_initial_issue_and_renewal_rotation|agent_certificate_lifecycle_rejects_invalid_transition|agent_certificate_lifecycle_rejects_invalid_material_without_leak|agent_certificate_lifecycle_snapshot_roundtrips_public_state_only|agent_certificate_lifecycle_restore_rejects_inconsistent_snapshot' crates/fleet-domain >/dev/null \
  || fail "agent certificate lifecycle state machine tests are missing"

rg -n 'agent_certificate_lifecycle_use_case_persists_issuance_and_audits|agent_certificate_lifecycle_use_case_rotates_with_grace_window|AgentCertificateLifecycleRepository' crates/fleet-application >/dev/null \
  || fail "agent certificate lifecycle application contract tests are missing"

rg -n 'agent_certificate_lifecycle_repository_roundtrips_public_state_only|agent_certificate_lifecycle.*private_key|agent_certificate_lifecycle.*certificate_body|agent_certificate_lifecycle.*websocket_handle' crates/fleet-store >/dev/null \
  || fail "agent certificate lifecycle store contract tests are missing"

for pattern in \
  'agent_certificate_lifecycle_update_roundtrips_public_metadata_only' \
  'agent_certificate_lifecycle_update_ignores_private_material_like_unknown_fields' \
  'agent_certificate_lifecycle_ack_roundtrips_public_status_only'
do
  rg -n "$pattern" crates/fleet-protocol >/dev/null \
    || fail "agent certificate lifecycle protocol tests are missing: $pattern"
done

for pattern in \
  'session_registry_records_agent_certificate_lifecycle_ack_for_matching_connection' \
  'agent_certificate_lifecycle_ack_from_wire_ignores_agent_mismatch' \
  'agent_certificate_lifecycle_ack_audit_omits_material' \
  'agent_certificate_lifecycle_update_rejects_until_runtime_support_exists' \
  'agent_certificate_lifecycle_dispatch_sends_public_update_to_connected_session' \
  'agent_certificate_lifecycle_dispatch_reports_not_connected_without_persisting_handles' \
  'agent_certificate_issuance_request_persists_and_dispatches_public_update_without_material' \
  'agent_certificate_issuance_request_requires_agent_write_permission' \
  'parses_agent_certificate_issuance_request_command' \
  'renders_agent_certificate_issuance_request_without_material' \
  'agent_certificate_lifecycle_status_returns_not_issued_for_known_agent_without_record' \
  'agent_certificate_lifecycle_status_returns_public_prefixes_without_material' \
  'parses_agent_certificate_lifecycle_status_command' \
  'renders_agent_certificate_lifecycle_status_without_material'
do
  rg -n "$pattern" crates/fleet-controller crates/fleet-cli >/dev/null \
    || fail "agent certificate lifecycle runtime boundary tests are missing: $pattern"
done

rg -n 'signing_key_rotation_cannot_activate_before_validation|signing_key_rotation_old_key_verifies_only_until_expiry|signing_key_rotation_errors_are_redacted|signer_rotation_policy_selects_new_signing_fingerprint_after_activation' crates >/dev/null \
  || fail "controller signing key rotation state machine tests are missing"

rg -n 'staged_rollout_skips_already_current_and_plans_bounded_batch|staged_rollout_ack_observed_allows_next_batch_and_completion|staged_rollout_timeout_fails_when_max_failures_is_exceeded|staged_rollout_completes_when_all_targets_are_already_current|staged_rollout_rejects_invalid_config_and_terminal_dispatch|staged_rollout_snapshot_roundtrips_in_flight_and_terminal_state|staged_rollout_restore_rejects_inconsistent_snapshot' crates >/dev/null \
  || fail "controller signing staged rollout state machine tests are missing"

rg -n 'signing_key_rotation_repository_roundtrips_without_private_material|signing_key_rotation_old_key_verification_window_survives_store_roundtrip|signing_key_rotation_use_case_saves_state_without_private_material|controller_task_signer_uses_explicit_signing_fingerprint_context' crates >/dev/null \
  || fail "controller signing key rotation persistence/signer boundary tests are missing"

rg -n 'controller_signing_staged_rollout_repository_contract_saves_public_state_only|staged_rollout_repository_roundtrips_without_material_or_handles|empty_database_initialization_records_schema_version' crates >/dev/null \
  || fail "controller signing staged rollout persistence boundary tests are missing"

rg -n 'request_signing_key_rotation_persists_state_and_security_audit|validate_signing_key_rotation_loads_state_saves_and_audits|activate_signing_key_rotation_uses_persisted_state_and_audits|retire_signing_key_rotation_rejects_before_guard_and_succeeds_after|fail_signing_key_rotation_records_terminal_failure_without_leaking_summary|invalid_signing_key_rotation_transition_does_not_save_or_audit_success' crates >/dev/null \
  || fail "controller signing key rotation operation/audit tests are missing"

rg -n 'valid_signing_material_pair_returns_public_fingerprint|mismatched_signing_material_pair_is_rejected|invalid_signing_material_error_does_not_echo_key_material|validate_signing_key_rotation_rejects_unrequested_fingerprint_without_save_or_audit' crates >/dev/null \
  || fail "controller signing key material validation tests are missing"

rg -n 'controller_signing_candidate_files_validate_and_return_expected_fingerprint|controller_signing_candidate_private_key_insecure_permissions_are_rejected_without_path_leak|controller_signing_candidate_rejects_active_or_transport_path_reuse_without_path_leak|controller_signing_key_file_swap_replaces_active_files_and_writes_backup|controller_signing_key_file_swap_rolls_back_after_public_swap_failure' crates >/dev/null \
  || fail "controller signing key filesystem staging/swap tests are missing"

rg -n 'controller_signing_runtime_identity_accepts_missing_rotation_state_as_active_steady|controller_signing_runtime_identity_rejects_invalid_active_material_without_leak|controller_signing_runtime_guard_accepts_matching_steady_state|controller_signing_runtime_guard_accepts_dual_trust_selected_new_key|controller_signing_runtime_guard_rejects_mismatched_active_material_without_leak|controller_signing_runtime_load_error_is_redacted_for_corrupt_persisted_state' crates >/dev/null \
  || fail "controller signing runtime bootstrap guard tests are missing"

rg -n 'legacy_pinned_controller_key_becomes_single_current_trust_bundle|controller_signing_trust_bundle_debug_does_not_expose_key_material|controller_trust_bundle_verifies_previous_key_within_window|controller_trust_bundle_rejects_previous_key_after_window|controller_trust_bundle_rejects_unknown_fingerprint|agent_config_builds_legacy_controller_signing_trust_bundle|trust_bundle_update_roundtrips_current_and_previous_entries|trust_bundle_update_ignores_private_material_like_unknown_fields|trust_bundle_ack_roundtrips_public_status_only|trust_bundle_update_rejects_duplicate_fingerprint_without_key_leak|trust_bundle_update_rejects_previous_entry_without_expiry|agent_controller_signing_trust_bundle_update_applies_in_memory_without_env|agent_controller_signing_trust_bundle_update_emits_ack_without_material|agent_controller_signing_trust_bundle_update_rejection_ack_is_bounded|agent_task_verification_uses_updated_bundle_and_preserves_task_guards|absent_controller_trust_bundle_sidecar_keeps_legacy_pinned_bundle|controller_trust_bundle_sidecar_roundtrips_public_fields_only|accepted_controller_trust_bundle_update_persists_for_restart|corrupt_controller_trust_bundle_sidecar_rejects_without_material_leak|persisted_controller_trust_bundle_verifies_task_after_restart' crates >/dev/null \
  || fail "agent controller signing trust bundle tests are missing"

rg -n 'signing_rotation_status_missing_state_reports_active_steady_readiness|signing_rotation_status_dual_trust_uses_prefixes_without_material_leak|signing_rotation_status_terminal_and_retirement_readiness_are_explicit|signing_rotation_status_reports_old_key_retirement_available_after_window|admin_can_get_controller_signing_rotation_status_without_material_leak|controller_signing_rotation_status_requires_admin_auth|parses_controller_signing_rotation_status_command|controller_signing_rotation_status_renderer_omits_key_material' crates >/dev/null \
  || fail "controller signing rotation status API/CLI tests are missing"

rg -n 'admin_can_request_controller_signing_rotation_without_material_leak|controller_signing_rotation_validate_rejects_key_body_fields_without_leak|admin_can_validate_controller_signing_rotation_from_candidate_paths|controller_signing_rotation_activate_retire_and_fail_are_state_machine_bound|controller_signing_rotation_fail_records_terminal_state_without_reason_leak|controller_signing_rotation_mutation_requires_write_permission|parses_controller_signing_rotation_mutation_commands' crates >/dev/null \
  || fail "controller signing rotation mutation API/CLI tests are missing"

rg -n 'controller_signing_rotation_restart_plan_reports_no_restart_for_steady_state|controller_signing_rotation_restart_plan_reports_restart_for_selected_mismatch|controller_signing_rotation_restart_plan_requires_admin_auth|parses_controller_signing_rotation_restart_plan_command|controller_signing_rotation_restart_plan_renderer_omits_key_material' crates >/dev/null \
  || fail "controller signing rotation restart-plan API/CLI tests are missing"

rg -n 'controller_signing_rotation_restart_action_audits_external_restart_without_material_leak|controller_signing_rotation_restart_action_rejects_when_restart_not_required_without_audit|controller_signing_rotation_restart_action_renderer_omits_key_material|controller_restart_service_dry_run_renders_systemctl_command' crates >/dev/null \
  || fail "controller signing rotation restart-action API/CLI tests are missing"

rg -n 'this version does not support in-process controller signing key reload|in-process hot reload/self-restart is not a current product path' crates docs >/dev/null \
  || fail "controller signing in-process reload no-go policy is missing"

rg -n 'must not self-restart|does not self-restart|HTTP handler self-restart.*not' crates docs >/dev/null \
  || fail "controller signing self-restart prohibition is missing"

rg -n 'admin_can_rollout_controller_signing_trust_bundle_to_connected_sessions_without_material_leak|session_registry_records_controller_signing_trust_ack_for_matching_connection|controller_trust_ack_from_wire_ignores_agent_mismatch|controller_signing_trust_bundle_ack_audit_omits_material|controller_signing_trust_bundle_rollout_skips_already_current_acknowledged_agent|controller_signing_trust_bundle_rollout_requires_restart_and_valid_state|controller_signing_trust_bundle_rollout_requires_write_permission|controller_signing_trust_bundle_retry_limits_batch_and_skips_disconnected_without_leak|controller_signing_trust_bundle_retry_rejects_zero_batch_without_material_leak|controller_signing_trust_bundle_staged_rollout_uses_ack_state_and_batch_limit|controller_signing_trust_bundle_staged_rollout_persists_waiting_state_between_ticks|controller_signing_staged_rollout_worker_continues_persisted_state_without_request_body|controller_signing_trust_bundle_staged_rollout_rejects_invalid_config_without_leak|controller_signing_trust_bundle_staged_rollout_requires_write_permission|controller_signing_staged_trust_bundle_request_uses_explicit_body|controller_signing_trust_bundle_staged_rollout_renderer_omits_key_material|controller_signing_trust_bundle_rollout_renderer_omits_key_material' crates >/dev/null \
  || fail "controller signing trust bundle rollout API/CLI tests are missing"

rg -n 'renderControllerSigningRotationStatus|buildStagedTrustBundleRequest|renderStagedTrustBundleResult|stageControllerSigningTrustBundle' web-admin/scripts/test.js web-admin/app.js web-admin/api-client.js >/dev/null \
  || fail "web admin controller signing staged rollout smoke coverage is missing"

rg -n 'runbook_execution_plan_renders_secret_template_with_explicit_resolver|secret_template_execution_omits_artifact_body_from_report' crates >/dev/null \
  || fail "secret-backed template rendering boundary tests are missing"

rg -n 'enrollment_token_create_is_audited_without_raw_token|agent_security_event_is_audited|auth_failure_writes_security_audit' crates >/dev/null \
  || fail "security/audit coverage tests are missing"

printf 'hardening audit ok\n'
