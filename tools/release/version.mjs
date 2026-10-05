// The versioned npm packages carry the release version. The crate has to report
// the same number, so one release ships one version of both artifacts; this is
// the only place that reads the packages and writes the crate manifest.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dirname, "..", "..");
const packagesDir = join(root, "packages");
const crateManifest = "crates/sandbox-toolkit/Cargo.toml";

const readPackageManifests = () =>
  readdirSync(packagesDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => join(packagesDir, entry.name, "package.json"))
    .map((path) => JSON.parse(readFileSync(path, "utf8")))
    .filter((manifest) => manifest.private !== true);

const readReleaseVersion = () => {
  const versions = new Set(
    readPackageManifests().map((manifest) => manifest.version),
  );

  if (versions.size !== 1) {
    throw new Error(
      `published packages must share one version, found ${[...versions]
        .toSorted((left, right) => left.localeCompare(right))
        .join(", ")}`,
    );
  }

  return [...versions][0];
};

const isBehind = (version, current) => {
  const numbers = (value) => value.split("-")[0].split(".").map(Number);
  const left = numbers(version);
  const right = numbers(current);

  for (let index = 0; index < 3; index += 1) {
    if (left[index] === right[index]) continue;
    return left[index] < right[index];
  }

  return false;
};

// The crate version is the `version` key of its `[package]` table, and nothing
// else in the manifest looks like it.
const crateVersionLine = (manifest) => {
  const lines = manifest.split("\n");
  const start = lines.indexOf("[package]");

  if (start < 0) throw new Error(`${crateManifest} has no [package] table`);

  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index].startsWith("[")) break;
    if (lines[index].startsWith('version = "')) return index;
  }

  throw new Error(`${crateManifest} [package] table has no version`);
};

const version = readReleaseVersion();

if (process.argv.includes("--print")) {
  console.log(version);
} else {
  const path = join(root, crateManifest);
  const manifest = readFileSync(path, "utf8");
  const lines = manifest.split("\n");
  const index = crateVersionLine(manifest);
  const current = /"(.*)"/.exec(lines[index])[1];

  if (isBehind(version, current)) {
    throw new Error(
      `the packages are at ${version}, behind the crate's ${current}: run \`pnpm exec changeset version\` first`,
    );
  }

  if (current === version) {
    console.log(`${crateManifest}: ${version} (unchanged)`);
  } else {
    lines[index] = `version = "${version}"`;
    writeFileSync(path, lines.join("\n"));
    console.log(`${crateManifest}: ${current} -> ${version}`);
  }
}
