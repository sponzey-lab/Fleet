#!/usr/bin/env node
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");

const root = join(__dirname, "..");
const gate = join(root, "scripts", "catalog_performance_gate.sh");
const temp = mkdtempSync(join(tmpdir(), "fleet-catalog-performance-"));
const runner = { label: "ubuntu-22.04", image_os: "ubuntu22", image_version: "20260830.1" };

function report(medianMs) {
  return {
    schema_version: 1,
    runner,
    rustc_version: "rustc 1.94.0",
    samples_ms: [medianMs, medianMs, medianMs, medianMs, medianMs, medianMs, medianMs],
    median_ms: medianMs,
  };
}

function write(name, payload) {
  const path = join(temp, name);
  writeFileSync(path, `${JSON.stringify(payload)}\n`);
  return path;
}

function run(args) {
  return execFileSync("sh", [gate, ...args], {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

try {
  const baseline = write("baseline.json", report(100));
  const passing = write("passing.json", report(150));
  assert.match(run(["verify", "--baseline", baseline, "--report", passing]), /gate ok/);

  const slower = write("slower.json", report(151));
  assert.throws(() => run(["verify", "--baseline", baseline, "--report", slower]), /regression/);

  const changedRunner = write("changed-runner.json", {
    ...report(100),
    runner: { ...runner, image_version: "20260831.1" },
  });
  assert.throws(
    () => run(["verify", "--baseline", baseline, "--report", changedRunner]),
    /runner metadata differs/,
  );
  assert.throws(
    () => run(["verify", "--baseline", join(temp, "missing.json"), "--report", passing]),
    /cannot read catalog performance baseline/,
  );
  process.stdout.write("catalog performance gate tests: PASS\n");
} finally {
  rmSync(temp, { recursive: true, force: true });
}
