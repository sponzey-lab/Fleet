# npm Trusted Publishing

Sponzey Fleet publishes npm packages from
`.github/workflows/npm-release.yml` using GitHub Actions OIDC. The release
workflow does not use or require a long-lived `NPM_TOKEN` secret.

## npm package configuration

Configure a Trusted Publisher separately in npm settings for every package:

- `@sponzey/fleet`
- `@sponzey/fleet-darwin-arm64`
- `@sponzey/fleet-darwin-x64`
- `@sponzey/fleet-linux-arm64`
- `@sponzey/fleet-linux-x64`

Use these publisher values for all five packages:

| Field | Value |
| --- | --- |
| Provider | GitHub Actions |
| Organization or user | `sponzey-lab` |
| Repository | `Fleet` |
| Workflow filename | `npm-release.yml` |
| Environment | Empty |
| Allowed action | `npm publish` |

The organization, repository, and workflow filename are identity constraints
and must match exactly. Each npm package can have only one Trusted Publisher.

## Workflow requirements

The publish job:

- runs on a GitHub-hosted runner;
- grants `id-token: write` and `contents: write` only to the publish job;
- uses Node.js 24 and npm 11, satisfying the Trusted Publishing minimums;
- declares the public source repository in every npm `package.json`;
- publishes the four platform packages before the wrapper package; and
- receives short-lived OIDC credentials automatically during each
  `npm publish` command.

Trusted Publishing applies only to npm publish operations. The local manual
publish helper still accepts `FLEET_NPM_TOKEN_FILE` because a developer shell
cannot assume the GitHub Actions OIDC identity.

## Release checklist

1. Confirm Trusted Publisher settings exist for all five packages.
2. Bump Cargo, wrapper, and platform package versions together.
3. Run `npm test --workspace @sponzey/fleet` and the release gate.
4. Commit the release metadata.
5. Push a new matching `v*.*.*` tag.
6. Confirm all platform builds and the final publish job succeed.
7. Verify the wrapper and platform versions on npm.

Do not move an already published release tag to adopt workflow changes. A
workflow rerun uses the commit referenced by that tag. Apply workflow or package
metadata corrections in a new release commit and publish a new version.

Official npm guidance:

- <https://docs.npmjs.com/trusted-publishers/>
- <https://docs.npmjs.com/generating-provenance-statements/>
