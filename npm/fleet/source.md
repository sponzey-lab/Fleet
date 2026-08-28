# npm fleet wrapper source index

The package resolves a native Fleet binary for npm installations. It owns
distribution packaging only; Rust code owns product behavior.

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `package.json` | Packaging | Wrapper package metadata, executable map and npm checks | npm install and release metadata |
| `bin/fleet` | Packaging | Resolves the explicit, development or platform Rust executable | Reads bootstrap-only `FLEET_*`; executes child binary |
| `scripts/source.md` | Tooling | Index for wrapper install and package verification scripts | Child tooling boundary |
| `README.md` | Packaging | User-facing npm wrapper install and distribution guidance | Mirrors executable/package contract |
