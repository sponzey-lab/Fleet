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
