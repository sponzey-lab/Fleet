# Security Checklist

- Enrollment tokens are one-time visible secrets.
- Bootstrap admin token maps to an authenticated admin actor and role before protected APIs run.
- Dangerous admin APIs check route permissions and return 403 with the required permission when denied.
- Agent identity is key-pair based.
- Controller identity is key-pair based.
- Agents pin the controller public key after enrollment.
- Task assignments use controller-signed envelopes.
- High-risk commands require explicit confirmation.
- HTTP transport is test-only, emits warnings, and writes Security audit when configured as the controller external URL.
- Product, customer, production, shared, and long-running environments use HTTPS.
- Product logs do not include command output or secret values.
