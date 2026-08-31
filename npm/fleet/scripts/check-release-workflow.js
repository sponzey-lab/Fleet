const fs = require("fs");
const path = require("path");

const workflowPath = path.join(
  __dirname,
  "..",
  "..",
  "..",
  ".github",
  "workflows",
  "npm-release.yml",
);
const workflow = fs.readFileSync(workflowPath, "utf8");

const requirements = [
  ["id-token: write", "publish job must grant id-token: write for npm OIDC"],
  ["npm install --global npm@11", "publish job must install npm 11 for Trusted Publishing"],
  ["package-manager-cache: false", "release setup must disable package manager caching"],
  ["RELEASE_SIGNING_PRIVATE_KEY", "release workflow must require the signing key secret"],
  ["./scripts/sign_release_sums.sh", "release workflow must sign SHA256SUMS"],
  ["test -f docs/release-signing-public.pem", "release workflow must fail when the public key is absent"],
  ["docs/release-signing-public.pem", "release workflow must publish the pinned release public key"],
  ["dist/release/SHA256SUMS.sig", "release workflow must upload the detached checksum signature"],
  ["- name: Sign release checksums\n        if: github.event_name == 'push' || github.event_name == 'workflow_dispatch'", "tagged releases and manual rehearsals must require the signing identity"],
  ["- name: Upload signed rehearsal artifact\n        if: github.event_name == 'workflow_dispatch' && success()", "manual signing rehearsal must retain its public verification evidence"],
  ["npm pack \"./dist/npm/$package\" --dry-run", "release dry-run must validate package contents without attempting to republish an existing version"],
];

for (const [fragment, message] of requirements) {
  if (!workflow.includes(fragment)) {
    console.error(message);
    process.exit(1);
  }
}

if (workflow.includes("secrets.NPM_TOKEN") || workflow.includes("NODE_AUTH_TOKEN")) {
  console.error("Trusted Publishing workflow must not depend on a long-lived npm token");
  process.exit(1);
}

console.log("npm Trusted Publishing workflow checks passed");
