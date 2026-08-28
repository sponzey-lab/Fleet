# npm fleet script source index

These Node scripts verify the wrapper distribution and create an npm global
launcher; they do not implement Fleet runtime behavior.

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `check-bin.js` | Test | Verifies the launcher and postinstall runtime-name contract | Spawns launcher with controlled environment |
| `check-version.js` | Test | Verifies package version alignment | Reads workspace manifests |
| `check-platform-packages.js` | Test | Verifies optional platform package metadata | Reads npm package manifests |
| `check-release-workflow.js` | Test | Verifies Trusted Publisher workflow contract | Reads workflow source |
| `postinstall.js` | Packaging | Creates safe global/PATH-visible `fleet` launcher when needed | Reads npm bootstrap environment; creates symlink/copy |
