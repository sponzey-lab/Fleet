# Web Admin Scripts Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `build.js` | Tooling | Exports checked static Web Admin assets. | Writes generated `dist` output. |
| `test.js` | Test | Verifies static UI, API client, OpenAPI/schema coverage, and remediation lifecycle controls. | Reads source and contract snapshots only. |
| `typecheck.js` | Tooling | Runs the browser JavaScript type-check contract. | Spawns the configured TypeScript checker. |
