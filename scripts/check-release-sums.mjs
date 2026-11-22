// Verify a directory of downloaded release assets against its checksum files.
// Node ESM, built-in modules only. The release-facts backstop runs this after a
// release is published; publish-release.ps1 performs the same checks before it
// publishes.
//
// Rules:
//   - SHA256SUMS.txt must exist and contain at least one hash;
//   - every archive asset (*.zip, *.tar.gz, *.exe) must be named by
//     SHA256SUMS.txt or its own .sha256 companion;
//   - the recorded digest must equal the file's actual SHA-256.

import { createHash } from "node:crypto";
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { join, basename } from "node:path";

const dir = process.argv[2];
if (!dir || !existsSync(dir) || !statSync(dir).isDirectory()) {
  console.log("FAIL release asset directory is missing");
  process.exit(1);
}

const files = readdirSync(dir).filter((name) => statSync(join(dir, name)).isFile());
const asset = (name) =>
  (name.endsWith(".zip") || name.endsWith(".tar.gz") || name.endsWith(".exe")) &&
  !name.endsWith(".sha256");

function parseSums(path) {
  const map = new Map();
  if (!existsSync(path)) return map;
  for (const line of readFileSync(path, "utf8").split(/\r?\n/)) {
    const m = line.match(/^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$/);
    if (m) map.set(m[2], m[1].toUpperCase());
  }
  return map;
}

const sumsPath = join(dir, "SHA256SUMS.txt");
const sums = parseSums(sumsPath);
let failed = false;

if (sums.size === 0) {
  console.log("FAIL SHA256SUMS.txt is missing or empty");
  failed = true;
} else {
  console.log(`PASS SHA256SUMS.txt carries ${sums.size} hash(es)`);
}

for (const name of files.filter(asset)) {
  let recorded = sums.get(name);
  if (!recorded) {
    const companion = join(dir, `${name}.sha256`);
    const companionSums = parseSums(companion);
    if (companionSums.has(name)) recorded = companionSums.get(name);
  }
  if (!recorded) {
    console.log(`FAIL ${name} has no checksum`);
    failed = true;
    continue;
  }
  const digest = createHash("sha256").update(readFileSync(join(dir, name))).digest("hex").toUpperCase();
  if (digest !== recorded) {
    console.log(`FAIL ${name} digest ${digest} does not match recorded ${recorded}`);
    failed = true;
  } else {
    console.log(`PASS ${name} digest matches`);
  }
}

if (failed) {
  console.log("RELEASE SUMS FAIL");
  process.exit(1);
}
console.log("RELEASE SUMS PASS");
