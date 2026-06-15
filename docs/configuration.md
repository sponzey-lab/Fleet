# Configuration Guide

Sponzey Fleet accepts external configuration only during process bootstrap.

Rules:

- Do not mutate process environment at runtime.
- Do not add runtime configuration mutation endpoints.
- Pass settings explicitly through typed `Settings`.
- HTTP controller URLs are allowed for test use, but every HTTP path must warn clearly and write the configured security audit event when the controller starts with an HTTP transport target.
- Product and production deployments must use HTTPS/TLS.
