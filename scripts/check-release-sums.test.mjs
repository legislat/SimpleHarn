// Falsifier tests for check-release-sums.mjs and the publish gate's companion
// rule (REVRPO-R8-01). Plain Node ESM: exit 0 on PASS, 1 on any failure.

import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const checker = join(here, "check-release-sums.mjs");
const publish = join(here, "publish-release.ps1");
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex").toUpperCase();

function run(dir) {
  return spawnSync(process.execPath, [checker, dir], { encoding: "utf8" });
}

function tempDir() {
  return mkdtempSync(join(tmpdir(), "release-sums-test-"));
}

let failed = 0;
function test(name, body) {
  try {
    body();
    console.log(`PASS ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`FAIL ${name}: ${error.message}`);
  }
}

test("an exactly named asset passes", () => {
  const dir = tempDir();
  try {
    const bytes = "asset-bytes";
    writeFileSync(join(dir, "pkg.tar.gz"), bytes);
    writeFileSync(join(dir, "SHA256SUMS.txt"), `${sha(bytes)}  pkg.tar.gz\n`);
    const result = run(dir);
    assert.equal(result.status, 0, result.stdout);
    assert.match(result.stdout, /PASS pkg\.tar\.gz digest matches/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("an exactly named companion passes", () => {
  const dir = tempDir();
  try {
    const bytes = "asset-bytes";
    writeFileSync(join(dir, "pkg.tar.gz"), bytes);
    writeFileSync(join(dir, "SHA256SUMS.txt"), `${sha("other")}  other.txt\n`);
    writeFileSync(join(dir, "pkg.tar.gz.sha256"), `${sha(bytes)}  pkg.tar.gz\n`);
    const result = run(dir);
    assert.equal(result.status, 0, result.stdout);
    assert.match(result.stdout, /PASS pkg\.tar\.gz digest matches/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a single-entry companion naming another file fails", () => {
  const dir = tempDir();
  try {
    const bytes = "asset-bytes";
    writeFileSync(join(dir, "pkg.tar.gz"), bytes);
    writeFileSync(join(dir, "SHA256SUMS.txt"), `${sha("other")}  other.txt\n`);
    writeFileSync(join(dir, "pkg.tar.gz.sha256"), `${sha(bytes)}  stale.tar.gz\n`);
    const result = run(dir);
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stdout, /FAIL pkg\.tar\.gz has no checksum/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("an uppercase archive extension is checked", () => {
  const dir = tempDir();
  try {
    writeFileSync(join(dir, "X.ZIP"), "asset-bytes");
    writeFileSync(join(dir, "SHA256SUMS.txt"), `${sha("other")}  other.txt\n`);
    const result = run(dir);
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stdout, /FAIL X\.ZIP has no checksum/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a wrong digest fails", () => {
  const dir = tempDir();
  try {
    writeFileSync(join(dir, "pkg.zip"), "asset-bytes");
    writeFileSync(join(dir, "SHA256SUMS.txt"), `${"0".repeat(64)}  pkg.zip\n`);
    const result = run(dir);
    assert.equal(result.status, 1, result.stdout);
    assert.match(result.stdout, /does not match recorded/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the publish gate keeps no single-entry fallback", () => {
  const text = readFileSync(publish, "utf8");
  assert.ok(
    !/companionSums\.Count\s+-eq\s+1/.test(text),
    "publish-release.ps1 still has the size-1 companion fallback"
  );
});

if (failed > 0) {
  console.log(`RELEASE SUMS TEST FAIL (${failed})`);
  process.exit(1);
}
console.log("RELEASE SUMS TEST PASS");
