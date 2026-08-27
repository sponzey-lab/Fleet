const fs = require("fs");
const path = require("path");

const packageJson = require("../package.json");
const cargoToml = fs.readFileSync(path.join(__dirname, "..", "..", "..", "Cargo.toml"), "utf8");
const versionMatch = cargoToml.match(/\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/);
const licenseMatch = cargoToml.match(/\[workspace\.package\][\s\S]*?\nlicense\s*=\s*"([^"]+)"/);
const repositoryMatch = cargoToml.match(
  /\[workspace\.package\][\s\S]*?\nrepository\s*=\s*"([^"]+)"/,
);
const expectedLicense = "AGPL-3.0-only";
const expectedCargoRepository = "https://github.com/sponzey-lab/Fleet";
const expectedNpmRepository = "git+https://github.com/sponzey-lab/Fleet.git";

if (!versionMatch) {
  console.error("workspace package version was not found in Cargo.toml");
  process.exit(1);
}

if (!licenseMatch || licenseMatch[1] !== expectedLicense) {
  console.error(`Cargo workspace license must be ${expectedLicense}`);
  process.exit(1);
}

if (!repositoryMatch || repositoryMatch[1] !== expectedCargoRepository) {
  console.error(`Cargo workspace repository must be ${expectedCargoRepository}`);
  process.exit(1);
}

if (packageJson.license !== expectedLicense) {
  console.error(`npm wrapper license must be ${expectedLicense}`);
  process.exit(1);
}

if (packageJson.repository?.url !== expectedNpmRepository) {
  console.error(`npm wrapper repository must be ${expectedNpmRepository}`);
  process.exit(1);
}

const cargoVersion = versionMatch[1];

if (packageJson.version !== cargoVersion) {
  console.error(
    `npm package version ${packageJson.version} does not match Cargo workspace version ${cargoVersion}`,
  );
  process.exit(1);
}

console.log(`npm package version matches Cargo workspace version: ${cargoVersion}`);
