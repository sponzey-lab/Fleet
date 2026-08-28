const fs = require("fs");
const os = require("os");
const path = require("path");

function pathEntries() {
  return (process.env.PATH || "")
    .split(path.delimiter)
    .filter(Boolean)
    .map((entry) => path.resolve(entry));
}

function inferGlobalBinDir() {
  if (process.env.npm_config_prefix) {
    return path.resolve(process.env.npm_config_prefix, "bin");
  }

  const marker = `${path.sep}lib${path.sep}node_modules${path.sep}`;
  const index = __dirname.indexOf(marker);
  if (index >= 0) {
    return path.resolve(__dirname.slice(0, index), "bin");
  }

  return null;
}

function isGlobalInstall() {
  const marker = `${path.sep}lib${path.sep}node_modules${path.sep}`;
  return process.env.npm_config_global === "true" || __dirname.includes(marker);
}

function platformPackageName() {
  const os = process.platform;
  const arch = process.arch;
  if (!["darwin", "linux"].includes(os)) {
    return null;
  }
  if (!["arm64", "x64"].includes(arch)) {
    return null;
  }
  return `fleet-${os}-${arch}`;
}

function platformBinaryCandidates(packageName) {
  return [
    path.resolve(__dirname, "..", "node_modules", "@sponzey", packageName, "bin", "fleet"),
    path.resolve(__dirname, "..", "..", packageName, "bin", "fleet"),
  ];
}

function warn(message) {
  console.warn(`WARNING: ${message}`);
}

function packageBinShim() {
  return path.resolve(__dirname, "..", "bin", "fleet");
}

function ensureGlobalBinShim(binDir) {
  if (!binDir || !isGlobalInstall()) {
    return null;
  }

  const source = packageBinShim();
  const target = path.resolve(binDir, "fleet");
  if (fs.existsSync(target)) {
    return target;
  }

  try {
    fs.mkdirSync(binDir, { recursive: true });
    fs.symlinkSync(source, target);
    return target;
  } catch (error) {
    try {
      fs.copyFileSync(source, target);
      fs.chmodSync(target, 0o755);
      return target;
    } catch (copyError) {
      warn(`could not create global fleet launcher at ${target}: ${copyError.message}`);
      warn(`original symlink error: ${error.message}`);
      return null;
    }
  }
}

function createLauncher(source, target) {
  if (fs.existsSync(target)) {
    return null;
  }

  try {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.symlinkSync(source, target);
    return target;
  } catch {
    try {
      fs.copyFileSync(source, target);
      fs.chmodSync(target, 0o755);
      return target;
    } catch {
      return null;
    }
  }
}

function safePathLauncherDirs(binDir) {
  const configured = (process.env.FLEET_POSTINSTALL_LINK_DIRS || "")
    .split(path.delimiter)
    .filter(Boolean)
    .map((entry) => path.resolve(entry));
  const known = new Set([
    ...configured,
    path.resolve("/usr/local/bin"),
    path.resolve("/opt/homebrew/bin"),
    path.resolve(os.homedir(), ".local", "bin"),
  ]);
  const prefixBin = binDir ? path.resolve(binDir) : "";
  return pathEntries().filter((entry) => entry !== prefixBin && known.has(entry));
}

function commandExistsOnPath(command) {
  return pathEntries().some((entry) => fs.existsSync(path.join(entry, command)));
}

function ensurePathVisibleLauncher(binDir) {
  if (!isGlobalInstall() || commandExistsOnPath("fleet")) {
    return null;
  }

  const source = packageBinShim();
  for (const dir of safePathLauncherDirs(binDir)) {
    const target = path.join(dir, "fleet");
    const launcher = createLauncher(source, target);
    if (launcher) {
      return launcher;
    }
  }
  return null;
}

const binDir = inferGlobalBinDir();
const launcher = ensureGlobalBinShim(binDir);
const binDirInPath = binDir && pathEntries().includes(binDir);
const pathLauncher = binDirInPath ? null : ensurePathVisibleLauncher(binDir);
if (binDir && !binDirInPath) {
  warn(`npm global bin is not in PATH: ${binDir}`);
  if (pathLauncher) {
    console.warn(`Created PATH-visible fleet launcher at: ${pathLauncher}`);
    console.warn("You can now run:");
    console.warn("  fleet --help");
  }
  if (launcher) {
    console.warn(`fleet launcher installed at: ${launcher}`);
    console.warn("Run it directly with:");
    console.warn(`  ${launcher} --help`);
  }
  console.warn("Add it before running fleet directly:");
  console.warn(`  export PATH="${binDir}:$PATH"`);
}

const packageName = platformPackageName();
if (packageName) {
  const hasPlatformBinary = platformBinaryCandidates(packageName).some((candidate) =>
    fs.existsSync(candidate),
  );
  if (!hasPlatformBinary) {
    warn(`platform binary package @sponzey/${packageName} was not found`);
    console.warn("Reinstall with optional dependencies enabled:");
    console.warn("  npm install -g @sponzey/fleet --include=optional");
  }
}
