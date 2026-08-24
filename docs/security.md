# Security Model

This document records the current security boundary for Sponzey Fleet. It is
implementation-facing documentation: code changes that affect authentication,
authorization, task dispatch, secrets, or audit should update this file.

## Current Scope

Sponzey Fleet currently has a bootstrap admin token and a minimal permission
model. Full OIDC, SSO, user management, and multi-admin session management are
Phase 6 work in `.tasks/plan.md`.

Current behavior:

- `sponzey controller init` prints one raw admin token.
- The raw admin token is shown once and is not stored in plaintext.
- The controller stores only the admin token hash.
- The stored bootstrap admin token maps to actor `bootstrap-admin`.
- The bootstrap admin actor has role `owner`.
- Protected REST APIs require `Authorization: Bearer <admin-token>`.
- The controller derives the request actor from the authenticated token, not
  from a request body field.
- Audit events for admin actions use the authenticated actor.

Phase 6 product admin identity must extend this model instead of replacing the
authorization boundary. OIDC sessions or API tokens must produce explicit actor
ids, roles, and project scope, but API handlers must still receive an
authenticated request context and must not trust UI-provided actor fields.

## Authentication Result

Admin authentication produces this controller-side request context:

```text
actor_id: stable admin actor id
role: owner | admin | operator | viewer
```

The request context is passed explicitly into the API route handling path. UI
state is not an authority. Request payload fields such as `created_by`,
`confirmed_by`, or approval `actor` are compatibility hints only; the controller
overrides or ignores them when an authenticated admin actor is available.

## Roles

| Role | Intent |
| --- | --- |
| `owner` | Bootstrap or organization owner. Full access. |
| `admin` | Operational admin. Full access in the current minimal model. |
| `operator` | Can operate jobs and approvals but cannot mint enrollment tokens or revoke agents. |
| `viewer` | Read-only operational visibility. |

`owner` and `admin` currently allow every defined permission. Phase 6 must split
these roles only after organization/user management, route-level authorization
tests, and audit actor attribution are implemented.

## Permissions

| Permission | Meaning |
| --- | --- |
| `agent_read` | List agents and read agent detail/snapshots. |
| `agent_write` | Change mutable agent metadata such as labels. |
| `agent_revoke` | Revoke an agent key and force re-enrollment. |
| `approval_read` | List approval requests. |
| `job_read` | List jobs and read job/output state. |
| `job_create` | Create command, runbook, and drift-check jobs. |
| `job_approve` | Approve, reject, or expire approval requests. |
| `job_cancel` | Cancel queued/running jobs. |
| `enrollment_token_read` | List enrollment token metadata. |
| `enrollment_token_create` | Create raw one-time enrollment tokens. |
| `enrollment_token_revoke` | Revoke enrollment tokens. |
| `audit_read` | Read audit events. |
| `policy_write` | Reserved for policy write APIs. |

## Permission Matrix

| Permission | owner | admin | operator | viewer |
| --- | --- | --- | --- | --- |
| `agent_read` | yes | yes | yes | yes |
| `agent_write` | yes | yes | no | no |
| `agent_revoke` | yes | yes | no | no |
| `approval_read` | yes | yes | yes | yes |
| `job_read` | yes | yes | yes | yes |
| `job_create` | yes | yes | yes | no |
| `job_approve` | yes | yes | yes | no |
| `job_cancel` | yes | yes | yes | no |
| `enrollment_token_read` | yes | yes | no | no |
| `enrollment_token_create` | yes | yes | no | no |
| `enrollment_token_revoke` | yes | yes | no | no |
| `audit_read` | yes | yes | yes | yes |
| `policy_write` | yes | yes | no | no |

## REST Error Contract

Protected API authentication and authorization errors are intentionally
separate:

```http
401 Unauthorized
{"error":"unauthorized"}
```

Use this when the admin token is missing or invalid.

```http
403 Forbidden
{"error":"forbidden","required_permission":"job_approve"}
```

Use this when the admin token is valid but the authenticated actor lacks the
permission needed by that route.

The Web Admin UI may display these errors, but it must not decide access. The
controller is the authority.

## Audit Export Boundary

The same `audit_read` permission covers recent audit list and cursor-based
audit export. Export supports category filtering and returns only redacted
values; `SecretRef` values are exposed as the marker `secret_ref`, never as raw
token or secret text.

The controller exposes no audit update or delete route. Audit events are
excluded from normal retention cleanup. The SQLite MVP store is append-only at
the application/API boundary, but it is not a tamper-proof WORM audit store.
Deployments that require long-term regulatory retention must export or back up
audit data into an external controlled store.

## Secret Provider Boundary

Secret reference handling exists as a typed contract, not as a production secret
manager integration.

- `SecretRef` accepts only validated `secret://...` references and rejects raw
  inline secret material, query strings, whitespace, unsupported schemes, and
  traversal-style paths.
- `SecretProvider` is an application-layer trait. Domain/application code must
  not depend on Vault/OpenBao/CyberArk SDKs or provider-specific clients.
- The current static provider is for tests/local contract verification. It must
  not become a runtime config editor or hidden production secret file.
- Resolved secrets use a wrapper whose `Display` and `Debug` output are
  redacted. Raw secret access is allowed only at the narrow render/runner call
  site that explicitly needs the value.
- Secret-backed template rendering requires explicit resolver/provider
  injection. Provider-free render paths continue to reject `SecretRef`
  variables.
- Secret-backed templates may write resolved content to the destination file,
  but rendered artifact body bytes are omitted by default. Controller artifact
  metadata, audit values, API responses, and Web Admin state must not carry raw
  rendered secret content.
- Secret provider bootstrap is represented by typed startup settings. The
  default provider mode is `disabled`; `static-test` accepts only a local JSON
  fixture path for contract tests and local development. Unsupported provider
  kinds and inline secret-like settings are rejected with redacted typed errors.
- Provider mode is decided at process start and then must be passed through
  explicit context/dependency injection. Request handlers, Web Admin actions,
  runbook jobs, and task execution paths must not change provider mode or read
  process environment to discover a provider.
- Controller bootstrap constructs the current provider dependency from typed
  settings. Disabled mode rejects every `SecretRef` through a typed denied
  result; `static-test` requires an explicit fixture source supplied by the
  local/test caller and is not read from environment or request state.
- Agent runbook planning adapts the selected provider into the runner's explicit
  resolver closure. The runner remains provider-agnostic and does not import
  application or infrastructure provider types.
- `sponzey apply` is validation-only and does not resolve secret-backed
  templates. It must not read provider config, fixture files, or raw secret
  material.
- Product Log, Field Debug Log, audit values, API responses, Web Admin state,
  and docs examples must contain secret references or redacted markers only.
- Product provider source configuration, provider lease/rotation lifecycle, and
  Vault/OpenBao adapters remain follow-up work.

## API Boundary Rules

API handlers should stay thin:

- Parse and validate input.
- Authenticate the admin token into an admin request context.
- Check the route permission before executing the use case.
- Pass the authenticated actor into the application use case.
- Write audit with that actor.

API handlers must not:

- Trust UI-provided actor fields for authorization or audit.
- Read process environment to decide authorization.
- Bypass the permission check for dangerous actions.
- Put raw secrets, command output, or tokens into product logs.

## Artifact Storage Credential Boundary

S3-compatible artifact storage credentials are not implemented yet. The accepted
boundary for that future adapter is:

- Artifact store backend selection is represented by typed immutable
  `ArtifactStoreSettings` and is resolved during Controller bootstrap only.
- The MVP default is local filesystem storage under
  `<data-dir>/controller/artifacts`; runtime API, Web Admin, request payload,
  and process env do not change that backend or root.
- Credentials are startup secrets or secret references, not request payloads.
- Web Admin must not expose an artifact storage settings editor.
- Controller handlers and application use cases must not read process
  environment variables to discover bucket, endpoint, access key, secret key, or
  session token.
- Product Log, Field Debug Log, audit values, job output, and Web Admin state
  must not contain raw access keys, secret keys, session tokens, signed URLs,
  object bodies, local paths, or destination paths.
- Any remote adapter must live behind the `ArtifactStore` infrastructure
  boundary and must not change Domain/Application dependencies.

## CLI Profile And Login

The CLI can use the bootstrap admin token directly or store it with the
controller endpoint through `sponzey login`. The default profile path is
`.sponzey/cli-profile.json`. The profile is treated as a secret and protected
remote commands reject group/world-readable profile files.

The stored credential still authenticates to the controller and produces the
same `actor_id + role` request context used by Web Admin and automation clients.

CLI profiles must not edit controller runtime configuration files directly.
They should select a controller endpoint and provide credentials for protected
API calls.

## Trust And Identity Separation

Phase 5 adds a typed trust settings boundary so transport identity and task
signing identity cannot be accidentally treated as the same material.

- TLS server identity is the HTTPS/WSS certificate and private key passed with
  `--tls-cert` and `--tls-key`. It proves the controller transport endpoint to
  clients and is validated at controller bootstrap.
- Controller signing identity is the Ed25519 key pair under the controller data
  directory. It signs task envelopes and is the identity pinned by agents after
  enrollment.
- Agent identity is the per-agent Ed25519 key pair generated during enrollment.
  It proves the agent session during WebSocket authentication.
- Agent client certificate trust is a future transport-auth layer. It must use a
  CA certificate path when enabled and must not replace agent key proof,
  controller-signed task envelopes, approval, expiry, nonce replay, or target
  validation.
- Until the controller listener enforces agent client certificates, explicit
  agent client CA input such as `--agent-client-ca-cert` must fail bootstrap
  with a non-leaking unsupported message rather than being silently accepted.

Required implementation rules:

- TLS server certificate/private key paths must be supplied together or not at
  all.
- TLS server private key and controller signing private key must be separate
  files.
- TLS server certificate and controller signing public key must be separate
  files.
- TLS certificate fingerprints must not be used as controller signing
  fingerprints.
- Trust settings are read only at bootstrap and passed as typed settings.
  Runtime handlers must not discover or mutate trust material through process
  environment, hidden globals, or config patch endpoints.
- Trust settings errors must not include raw key paths, secret-like filenames,
  key contents, or certificate bodies.

## Agent Client Certificate Lifecycle Policy

Agent client certificate lifecycle is a domain/application concern that must
remain separate from TLS listener enforcement. The current domain foundation
models only public certificate metadata: agent id, certificate serial,
certificate fingerprint, validity window, lifecycle state, grace window, and
revocation reason. It does not store or log private keys, PEM bodies,
certificate file paths, CA paths, process environment values, or raw request
payloads.

States:

- `not_issued`
- `issuance_requested`
- `issued`
- `renewal_requested`
- `dual_certificate_active`
- `revoked`
- `expired`
- `failed`

Rules:

- Initial issuance must be requested before a certificate can be issued.
- The current public controller surfaces are limited to admin protected
  read-only status and issuance request:
  `GET /api/agents/{agent_id}/certificate-lifecycle/status`,
  `sponzey agents certificate-status <agent-id>`,
  `POST /api/agents/{agent_id}/certificate-lifecycle/request-issuance`, and
  `sponzey agents request-certificate-issuance <agent-id>`.
- The lifecycle status surface requires `agent_read`, returns `not_issued` for
  a known agent without lifecycle state, and exposes only public state,
  fingerprint prefixes, timestamps, and bounded reason values.
- The issuance request surface requires `agent_write`, verifies the agent
  exists, stores lifecycle state through `RequestAgentCertificateIssuance`,
  writes Security audit, and sends only public lifecycle metadata to an
  authenticated connected session.
- Dispatch success means the controller queued an update to the session; it
  does not mean the agent accepted, installed, or began using a certificate.
- Issued certificates must be valid at the issuance operation time.
- Renewal can be requested only when the issued certificate is inside the
  configured renewal window.
- Renewal activation requires a next certificate with a distinct serial and
  fingerprint from the current certificate.
- During `dual_certificate_active`, the current certificate is trusted only
  until the explicit rotation grace deadline; the next certificate is trusted
  according to its validity window.
- Rotation completion cannot happen before the grace deadline.
- Revoked, expired, and failed lifecycle states are terminal.
- Lifecycle errors must not echo certificate bodies, private key material,
  secret-like input, or local filesystem paths.

`fleet-application` exposes the repository trait and use cases for issuance
request, issue, renewal request, renewal activation, and rotation completion.
Those use cases save lifecycle snapshots and write Security audit events with
state names only. `fleet-store` persists the snapshot in
`agent_certificate_lifecycle` using public metadata columns only.
`fleet-protocol` defines `agent_certificate_lifecycle_update` and
`agent_certificate_lifecycle_ack` as public-only lifecycle messages. The
current controller runtime records ack observations in session runtime state
and Security audit only. The current agent runtime rejects lifecycle updates
with `certificate_lifecycle_runtime_not_implemented`; it does not install,
trust, reload, or store certificate material. The controller has an internal
dispatch helper that sends public-only lifecycle updates from application
lifecycle records to authenticated connected sessions. Future runtime handlers
or public surfaces that change lifecycle state must call the application use
cases. WebSocket handlers, REST handlers, and UI code must not duplicate
transition rules or write lifecycle state directly.

## Controller Signing Key Rotation Policy

Controller signing key rotation is modeled separately from TLS certificate
rotation. The domain state machine uses public fingerprints and time windows
only; private key material, PEM bodies, key file paths, and process environment
values do not enter domain state.

States:

- `steady`
- `rotation_requested`
- `new_material_validated`
- `dual_trust_active`
- `old_key_retired`
- `rotation_failed`
- `canceled_before_activation`

Rules:

- Old and new signing fingerprints must be distinct.
- New material validation must happen after the rotation request.
- Dual trust cannot activate before new material validation.
- After dual trust activation, the new key signs new tasks.
- The old key verifies only tasks signed before activation and only until
  `old_key_verifies_until`.
- The old key cannot retire before the dual-trust verification window expires.
- Terminal failure/cancel/retired states cannot activate a rotation later.
- Rotation state persistence stores only controller id, state, public
  fingerprints, validity timestamps, and update time. It must not store private
  key material, private key paths, PEM bodies, or TLS certificate material.
- Application rotation operations own request, validation, activation,
  retirement, and failure orchestration. Interface handlers must call these use
  cases instead of mutating persisted rotation state directly.
- New controller signing material must pass keypair validation before
  `new_material_validated`: the candidate private key signs an explicit
  challenge, the candidate public key verifies it, and the derived public
  fingerprint must match the pending rotation fingerprint.
- Candidate controller signing key files are validated through the controller
  filesystem boundary before replacement. Candidate files must be separate from
  active signing files and transport/TLS key files; candidate private key
  permissions must reject group/other access.
- Active controller signing key file replacement uses explicit paths, backup
  files, temporary replacement files, post-swap verification, and rollback on
  partial failure. Swap completion does not activate dual trust by itself; the
  application rotation operation still owns state transition.
- Controller bootstrap validates the active signing public/private material by
  challenge, loads persisted rotation state, and fail-closes if the active
  public fingerprint does not match the domain-selected current signing
  fingerprint. Missing persisted rotation state is treated as active steady
  state until an explicit rotation operation creates a record.
- Rotation operation audit events record transition action, controller id,
  public fingerprint prefixes, and timestamps only. Failure summaries are
  redacted before audit/log emission and must not include private key paths,
  PEM bodies, or task payloads.
- Controller signer selection must receive the selected signing fingerprint as
  explicit context. It must not infer the signing key from process environment,
  mutable global state, or the TLS certificate fingerprint.
- Agent-side controller signing trust is represented as an explicit trust
  bundle containing public fingerprints, public keys, roles, and verification
  windows only. Legacy enrolled agents are adapted to a one-entry `current`
  bundle from their pinned signing fingerprint and current controller signing
  public key.
- Controller-to-agent trust bundle update protocol messages carry only public
  fingerprints, public keys, `current`/`previous` roles, and validity
  timestamps. Agent sessions validate the payload into the domain trust bundle
  model and store the accepted bundle in explicit in-memory session state for
  subsequent task verification. Accepted updates are also persisted to an agent
  config sidecar containing only the same public trust metadata so agent
  restart does not fall back to an obsolete pinned key during rotation. The
  message and sidecar must not carry private key material, key paths, TLS
  certificate material, or task payload bodies.
- Trust bundle verification must run after target/expiry validation and before
  nonce acceptance leads to execution. The verifier must accept the `current`
  entry for signatures issued inside its validity window, accept a `previous`
  entry only for envelopes issued before rotation and verified before its
  expiry, reject unknown fingerprints, and reject expired trust entries.
- Agent trust bundles must be passed as explicit objects into the task verifier.
  They must not be discovered through runtime environment reads, hidden globals,
  TLS certificate fingerprints, or runtime config patch endpoints.
- Agent trust bundle sidecar load happens once during bootstrap from the
  explicit agent config directory. Corrupt or invalid sidecar content is a
  startup/config error and must not leak raw public key bodies or
  private-material-like values.
- Controller signing rotation status is exposed through a read-only
  admin-authenticated API and CLI command.
  The status query may return controller id, persisted state name, readiness
  name, fingerprint prefixes, dual-trust timestamps, bootstrap guard summary,
  and agent trust rollout summary.
- Rotation status queries must not transition rotation state, read private key
  files, read TLS certificate files, inspect task payload bodies, or discover
  configuration through process environment at handler time.
- Rotation status responses and CLI output must not include private key
  material, private key paths, raw public key bodies, TLS certificate material,
  TLS key paths, task payload bodies, or secret-like diagnostic dumps.
- Controller signing rotation mutation API and CLI commands call the existing
  application request/validate/activate/retire/fail use cases. HTTP handlers
  must not duplicate transition logic or write persisted state directly.
- Mutation routes require `signing_rotation_write` permission. Request body
  actor fields are ignored; authenticated admin context supplies the audit
  actor.
- Mutation DTOs accept public fingerprints, old-key verification window values,
  redacted reason text, and controller-local candidate file paths only. They
  must reject private key bodies, raw public key bodies, PEM bodies, TLS
  certificate material, task payload bodies, and unknown secret-like fields.
- Candidate material validation reads explicit candidate file paths through the
  controller filesystem boundary. It must not read process environment,
  hidden global config, or UI-provided runtime config patches.
- Activation changes rotation state only. It must not implicitly swap key
  files, reload process state, or mutate process environment. Operators must
  use status `bootstrap_guard` and the restart-plan API/CLI to confirm the
  active runtime key matches persisted selected state.
- Controller signing rotation restart-plan API and CLI are read-only operator
  guidance surfaces. They may report `restart_required`, `reload_supported`,
  recommended action, verification commands, and safety notes, but must not
  restart the controller, reload key files, swap material, patch runtime
  config, or expose key paths/material.
- Controller signing rotation restart-action API and CLI record an audited
  external service-manager restart intent. The API returns explicit local
  service commands and verification commands but must not self-restart the HTTP
  handler, invoke systemd, reload key files, or mutate runtime config.
- Controller signing trust bundle rollout API and CLI dispatch public-only
  `controller_signing_trust_bundle_update` messages to authenticated connected
  agent sessions. Rollout requires active runtime signer to match persisted
  selected signer and must not create or modify Job/Assignment state.
- Agent trust bundle acknowledgement uses public-only
  `controller_signing_trust_bundle_ack` messages. The controller records the
  ack only in connection-scoped session registry runtime state and may skip a
  later rollout as `skipped_already_current` when the accepted current
  fingerprint matches. Ack audit values contain only accepted status, entries
  count, current fingerprint prefix, and a bounded reason code.
- Controller signing trust bundle retry API and CLI reuse the same public-only
  construction boundary with an explicit `max_agent_count` batch limit. Retry
  uses only observable session registry state and explicit operator input; it
  must not infer hidden key paths or store WebSocket handles in persisted state.
- Trust bundle rollout may read an explicit controller-local previous public
  key path for dual-trust payload construction. The path and public key bodies
  must not be returned, logged, audited, stored as domain state, or confused
  with private key material.

Audit/log policy:

- Product Log may record rotation requested, activated, retired, and failed
  summaries.
- Field Debug Log may record transition names, guard failure reasons, and
  validity window timestamps.
- No log level may record private key material, key file content, task payload
  body, or secret-like key paths.

## Current Limits

- There is no OIDC/SAML/SSO integration. Phase 6 owns OIDC login, session
  lifecycle, API token model, project/team RBAC, and route permission tests.
- There is no product-grade multi-admin user lifecycle. Phase 6 must add
  explicit admin identity, project membership state, and audit actor tests.
- The current SQLite admin token table is a bootstrap foundation, not a full
  user store.
- `owner` and `admin` are intentionally equivalent for now.
- Permission checks cover the current REST route boundary. Every new public
  API must add an explicit permission and table-driven authorization test before
  becoming public.

## Post-MVP Security Phase Map

| Area | Phase | Required boundary |
| --- | --- | --- |
| mTLS and agent certificate rotation | Phase 5 | Typed trust settings separate TLS server identity, agent identity, controller signing identity, and future agent client certificate trust. Explicit agent client CA bootstrap input is rejected until listener enforcement exists. Agent certificate lifecycle domain state machine, snapshot/restore boundary, application repository/use-case contract, SQLite/Postgres-shaped public metadata persistence foundation, public-only lifecycle update/ack protocol schema, controller ack observation/audit, internal controller update dispatch helper, admin protected status and issuance request API/CLI surfaces, and agent explicit rejection ack exist. Full mTLS listener enforcement, issue/renew/activate/revoke public controller surfaces, agent-side certificate application, revocation propagation, and runtime trust enforcement remain. |
| Controller signing key rotation | Phase 5 | Domain state machine, dual-trust decision policy, SQLite/Postgres-shaped persistence contract, application operation/audit boundary, signing material validation boundary, filesystem staging/swap boundary, bootstrap runtime guard, explicit signer selection context, agent-side trust bundle verification, trust-bundle update/ack protocol and session foundation, agent trust sidecar restart survival, read-only status API/CLI, mutation API/CLI, restart-plan API/CLI, audited external restart-action API/CLI, admin-triggered trust-bundle rollout API/CLI, bounded retry coordinator API/CLI, already-current ack skip, staged rollout domain state machine/persistence/worker, and Web Admin staged rollout surface exist. In-process hot reload/self-restart is not a current product path; audited external restart-action is the supported path. |
| Secret provider boundary | Phase 5 | Typed `SecretRef`, application `SecretProvider` trait, static fake provider, disabled provider, explicit runner resolver injection, typed startup `SecretProviderSettings`, controller bootstrap provider factory, agent runbook resolver handoff, and redaction tests exist; Vault/OpenBao adapter only after provider integration tests. |
| OIDC/admin identity/project RBAC | Phase 6 | Authenticated context, project scope, permission matrix, audit actor contract. |
| Git sync credentials | Phase 7 | SecretRef only; validation/activation audit; no raw Git token in logs. |
| Notification webhooks | Phase 8 | SecretRef webhook credentials and redacted summary payloads. |
| Agent update policy | Phase 9 | Signed artifact verification before install, rollback state machine. |
| HA coordination | Phase 10 | Lease/claim tests; no WebSocket handle persisted. |
| Compliance audit hardening | Phase 11 | Tamper-evident chain and signed export manifest without claiming WORM storage. |
| Plugin/external adapter | Phase 14 | Disabled by default, signed manifest, approval/capability/audit required. |
