# Security Model

This document records the current security boundary for Sponzey Fleet. It is
implementation-facing documentation: code changes that affect authentication,
authorization, task dispatch, secrets, or audit should update this file.

## Current Scope

Sponzey Fleet currently has a bootstrap admin token and a minimal permission
model. This is not full OIDC, SSO, user management, or multi-admin session
management yet.

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

Future product admin identity should extend this model instead of replacing the
authorization boundary. A later login/profile flow can issue admin sessions or
API tokens with explicit actor ids and roles, but API handlers must still
receive an authenticated request context and must not trust UI-provided actor
fields.

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

`owner` and `admin` currently allow every defined permission. This can be
split later when organization/user management exists.

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

## CLI Profile And Login Direction

The current CLI can use the bootstrap admin token directly. A future `login`
flow should keep only a controller profile and a scoped admin credential. That
credential should still authenticate to the controller and produce the same
`actor_id + role` request context used by Web Admin and automation clients.

CLI profiles must not edit controller runtime configuration files directly.
They should select a controller endpoint and provide credentials for protected
API calls.

## Current Limits

- There is no OIDC/SAML/SSO integration yet.
- There is no product-grade multi-admin user lifecycle yet.
- The current SQLite admin token table is a bootstrap foundation, not a full
  user store.
- `owner` and `admin` are intentionally equivalent for now.
- Permission checks cover the current REST route boundary. Future APIs must add
  explicit permissions before becoming public.
