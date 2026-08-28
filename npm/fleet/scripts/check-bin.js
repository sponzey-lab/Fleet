const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const bin = path.join(__dirname, "..", "bin", "fleet");
const legacyBin = path.join(__dirname, "..", "bin", "sponzey");
const postinstall = path.join(__dirname, "postinstall.js");
const packageJson = require(path.join(__dirname, "..", "package.json"));

if (!fs.existsSync(bin)) {
  console.error("missing bin/fleet");
  process.exit(1);
}

if (fs.existsSync(legacyBin)) {
  console.error("legacy bin/sponzey executable must not remain after the fleet hard cut");
  process.exit(1);
}

if (packageJson.scripts?.postinstall !== "node ./scripts/postinstall.js") {
  console.error("package.json must run scripts/postinstall.js after npm install");
  process.exit(1);
}

if (!fs.existsSync(postinstall)) {
  console.error("missing scripts/postinstall.js");
  process.exit(1);
}

const body = fs.readFileSync(bin, "utf8");

if (!body.startsWith("#!/usr/bin/env sh")) {
  console.error("bin/fleet must be a portable shell shim");
  process.exit(1);
}

if (!body.includes("target/debug/fleet")) {
  console.error("bin/fleet must point to the Rust development binary");
  process.exit(1);
}

if (!body.includes("FLEET_BIN")) {
  console.error("bin/fleet must support explicit binary override for local pack smoke");
  process.exit(1);
}

if (body.includes("SPONZEY_FLEET_")) {
  console.error("bin/fleet must not retain the previous runtime override prefix");
  process.exit(1);
}

if (!body.includes("fleet-$PLATFORM_OS-$PLATFORM_ARCH")) {
  console.error("bin/fleet must support platform binary package lookup");
  process.exit(1);
}

const legacyOverride = spawnSync(bin, ["--help"], {
  env: {
    ...process.env,
    FLEET_NPM_OS: "plan9",
    FLEET_NPM_ARCH: "mips",
    SPONZEY_FLEET_BIN: "/does-not-exist",
  },
  encoding: "utf8",
});

if (legacyOverride.status !== 127 || !legacyOverride.stderr.includes("unsupported platform")) {
  console.error("previous runtime overrides must not affect bin/fleet");
  process.exit(1);
}

if (!body.includes("node_modules/@sponzey/fleet-$PLATFORM_OS-$PLATFORM_ARCH")) {
  console.error("bin/fleet must support npm nested optional dependency lookup");
  process.exit(1);
}

const unsupported = spawnSync(bin, ["--help"], {
  env: {
    ...process.env,
    FLEET_NPM_OS: "plan9",
    FLEET_NPM_ARCH: "mips",
  },
  encoding: "utf8",
});

if (unsupported.status !== 127) {
  console.error(`unsupported platform should exit 127, got ${unsupported.status}`);
  process.exit(1);
}

if (!unsupported.stderr.includes("unsupported platform for @sponzey/fleet")) {
  console.error("unsupported platform error message is missing");
  process.exit(1);
}

const prefix = fs.mkdtempSync(path.join(os.tmpdir(), "sponzey-postinstall-"));
const pathBin = fs.mkdtempSync(path.join(os.tmpdir(), "sponzey-path-bin-"));
const postinstallRun = spawnSync(process.execPath, [postinstall], {
  env: {
    ...process.env,
    npm_config_global: "true",
    npm_config_prefix: prefix,
    PATH: pathBin,
    FLEET_POSTINSTALL_LINK_DIRS: pathBin,
  },
  encoding: "utf8",
});

if (postinstallRun.status !== 0) {
  console.error(`postinstall should not fail, got ${postinstallRun.status}`);
  process.exit(1);
}

if (!postinstallRun.stderr.includes("npm global bin is not in PATH")) {
  console.error("postinstall must warn when npm global bin is not in PATH");
  process.exit(1);
}

const installedLauncher = path.join(prefix, "bin", "fleet");
if (!fs.existsSync(installedLauncher)) {
  console.error("postinstall must create a global fleet launcher when npm did not");
  process.exit(1);
}

if (!postinstallRun.stderr.includes("fleet launcher installed at")) {
  console.error("postinstall must show the installed launcher path");
  process.exit(1);
}

const pathVisibleLauncher = path.join(pathBin, "fleet");
if (!fs.existsSync(pathVisibleLauncher)) {
  console.error("postinstall must create a PATH-visible fleet launcher when possible");
  process.exit(1);
}

if (!postinstallRun.stderr.includes("Created PATH-visible fleet launcher at")) {
  console.error("postinstall must show the PATH-visible launcher path");
  process.exit(1);
}

console.log("bin/fleet wrapper checks passed");
