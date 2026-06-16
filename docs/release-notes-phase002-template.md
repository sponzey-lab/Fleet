# Sponzey Fleet Phase 002 Release Notes Template

Use this template for beta releases after MVP.

## Highlights

- Remote agent enrollment supports HTTPS/WSS. HTTP remains available only for setup checks, local development, lab tests, and short-lived validation, with insecure transport warnings.
- The npm wrapper installs a single `sponzey` binary selected by platform package.
- Standalone binary tarballs and `SHA256SUMS` are attached to the GitHub Release.

## Upgrade Notes

- Re-enroll agents when the controller signing fingerprint changes.
- Existing loopback demo data directories can be kept, but production agents should use a persistent data directory such as `/var/lib/sponzey-fleet`.
- Linux binaries are built on Ubuntu 22.04 and must not require a glibc version newer than `GLIBC_2.35`.

## Known Limitations

- Built-in HTTPS supports PEM certificate/key material; automated certificate renewal is outside this release.
- Controller signing key rotation is not automated yet.
- Approval request lifecycle APIs are available. Dedicated Web Admin approval queue and RBAC/admin identity separation remain follow-up work.
- Linux service install/remove writes systemd files only when run as root on Linux.

## Verification

- `cargo test --workspace`
- `npm test --workspace @sponzey/fleet`
- `npm test --workspace web-admin`
- `./scripts/smoke_mvp.sh`
- `./scripts/smoke_remote_tls_loopback.sh`
- `./scripts/check_linux_glibc_baseline.sh <linux-binary>`
