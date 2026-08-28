# Security Checklist

- Enrollment tokens are one-time visible secrets.
- Bootstrap admin token maps to an authenticated admin actor and role before protected APIs run.
- Dangerous admin APIs check route permissions and return 403 with the required permission when denied.
- Agent identity is key-pair based.
- Controller identity is key-pair based.
- Agents pin the controller public key after enrollment.
- Task assignments use controller-signed envelopes.
- A successful remediation creates at most one signed verification assignment through the durable Job/assignment/correlation/audit transaction; dispatch occurs only after commit. Before listener readiness, one bounded recovery scan reconciles only correlation-free pending records and audits unverifiable legacy rows without dispatching them. Resolution requires both a successful verification assignment and fresh compliant persisted evidence after remediation execution; the atomic store write rechecks correlation and resolves only the origin drift.
- High-risk commands require explicit confirmation.
- HTTP transport is test-only, emits warnings, and writes Security audit when configured as the controller external URL.
- Product, customer, production, shared, and long-running environments use HTTPS.
- Product logs do not include command output or secret values.
