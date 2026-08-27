const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "..", "..");
const wrapperPackage = require("../package.json");
const expectedLicense = "AGPL-3.0-only";
const expectedRepository = "git+https://github.com/sponzey-lab/Fleet.git";

const expected = [
  ["@sponzey/fleet-darwin-arm64", "darwin", "arm64"],
  ["@sponzey/fleet-darwin-x64", "darwin", "x64"],
  ["@sponzey/fleet-linux-arm64", "linux", "arm64"],
  ["@sponzey/fleet-linux-x64", "linux", "x64"],
];

for (const [name, os, cpu] of expected) {
  const version = wrapperPackage.optionalDependencies[name];
  if (version !== wrapperPackage.version) {
    console.error(`${name} optional dependency must match wrapper version ${wrapperPackage.version}`);
    process.exit(1);
  }

  const dir = name.replace("@sponzey/", "");
  const packageJsonPath = path.join(root, dir, "package.json");
  if (!fs.existsSync(packageJsonPath)) {
    console.error(`missing platform package: ${packageJsonPath}`);
    process.exit(1);
  }

  const packageJson = require(packageJsonPath);
  if (packageJson.name !== name) {
    console.error(`${dir} package name mismatch: ${packageJson.name}`);
    process.exit(1);
  }
  if (packageJson.version !== wrapperPackage.version) {
    console.error(`${dir} package version must match wrapper version ${wrapperPackage.version}`);
    process.exit(1);
  }
  if (!Array.isArray(packageJson.os) || packageJson.os[0] !== os) {
    console.error(`${dir} must declare os ${os}`);
    process.exit(1);
  }
  if (!Array.isArray(packageJson.cpu) || packageJson.cpu[0] !== cpu) {
    console.error(`${dir} must declare cpu ${cpu}`);
    process.exit(1);
  }
  if (!Array.isArray(packageJson.files) || !packageJson.files.includes("bin")) {
    console.error(`${dir} must publish the bin directory`);
    process.exit(1);
  }
  if (packageJson.license !== expectedLicense) {
    console.error(`${dir} license must be ${expectedLicense}`);
    process.exit(1);
  }
  if (packageJson.repository?.url !== expectedRepository) {
    console.error(`${dir} repository must be ${expectedRepository}`);
    process.exit(1);
  }
}

console.log("platform optional package checks passed");
