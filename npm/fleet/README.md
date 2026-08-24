# @sponzey/fleet

This package is a distribution wrapper for the Rust `sponzey` binary.

The repository builds the binary with Cargo:

```sh
cargo build -p fleet-cli
npm test --prefix npm/fleet
```

The `sponzey` bin shim does not start a Node.js runtime server. It only resolves and executes a Rust binary.

Resolution order:

1. Explicit local binary override: `SPONZEY_FLEET_BIN`
2. Repository development binary: `target/debug/sponzey`
3. Release binary package next to this package, using `@sponzey/fleet-<os>-<arch>`

Supported release package targets:

- `@sponzey/fleet-darwin-arm64`
- `@sponzey/fleet-darwin-x64`
- `@sponzey/fleet-linux-arm64`
- `@sponzey/fleet-linux-x64`

Windows packages are not currently published. Unsupported platforms fail with a
clear `unsupported platform` error and exit code `127`.

Local pack smoke:

```sh
./scripts/npm_local_pack_smoke.sh
```

Local platform package install smoke:

```sh
./scripts/npm_platform_local_install_smoke.sh
```

This stages the current Rust binary into the current OS/architecture package, packs the wrapper and platform package, creates a temporary global npm-style symlink layout, and verifies that `sponzey --help` resolves through the platform package.

Local demo smoke:

```sh
./scripts/npm_demo_smoke.sh
```

The demo starts a temporary loopback-only controller, enrolls a local demo agent, runs a small confirmed command, prints the `/admin` URL, and removes the temporary data directory unless `sponzey demo --keep-temp` is used.

Registry publish for the current OS/architecture:

```sh
./scripts/npm_publish_current_platform.sh --dry-run
SPONZEY_NPM_TOKEN_FILE=token.md ./scripts/npm_publish_current_platform.sh
./scripts/manual_npm_registry_smoke.sh
```

The publish script stages the current `target/release/sponzey` binary into the matching platform package, publishes that package first, then publishes this wrapper package. The wrapper package is what users install:

```sh
npm install -g @sponzey/fleet
sponzey --help
```

The installer creates the npm global `sponzey` launcher when possible. It also
tries to create a PATH-visible launcher in a safe writable bin directory such as
`/usr/local/bin`. It does not silently edit shell profile files. If `sponzey` is
still not found, add the npm global bin directory explicitly:

```sh
export PATH="$(npm prefix -g)/bin:$PATH"
```

Standalone release archives are the non-npm install path. GitHub release
artifacts are named `sponzey-<os>-<arch>.tar.gz` and are published with
`SHA256SUMS`. Verify them before installing, and verify `SHA256SUMS.sig` with
the pinned release public key when a signature is published:

```sh
./scripts/verify_standalone_artifacts.sh dist/release
./scripts/verify_release_signature.sh dist/release ./release-public-key.pem
```

For Linux services, install the resolved Rust binary rather than an npm shim:

```sh
sponzey controller install-service --data-dir /var/lib/sponzey-fleet --dry-run
sponzey agent install-service --data-dir /var/lib/sponzey-fleet --dry-run
sponzey controller status-service --dry-run
sponzey agent logs-service --dry-run
```

Upgrade is currently an external npm/artifact operation. Inspect the supported
policy first:

```sh
sponzey upgrade --dry-run
```

GitHub Actions release:

1. Add an npm automation token as the repository secret `NPM_TOKEN`.
2. Bump `Cargo.toml`, `Cargo.lock`, root `package.json`, `npm/fleet/package.json`, and every `npm/fleet-*/package.json` to the same version.
3. Push a matching tag, for example `v0.0.15`.

The `.github/workflows/npm-release.yml` workflow builds native platform packages on GitHub-hosted runners, publishes all platform packages first, and publishes this wrapper package last.
Linux release binaries are built on Ubuntu 22.04 to avoid requiring glibc 2.39 from Ubuntu 24.04.
