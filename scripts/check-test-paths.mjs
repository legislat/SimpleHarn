// Test-source machine-path scanner. Node only, no external dependencies.
// Public test and source trees (src, tests, examples) must carry no operator
// machine paths: string-only fixtures use neutral relative paths, and the card
// suites read model paths from SIMPLEHARN_T_* environment variables whose
// defaults live only in the private handoff preflight scripts.
//
// The ban set is the scoped `testSourceBans` array in scripts/machine-path-bans.json
// (regular-expression source text, matched case-insensitively, the same way
// check-docs.mjs reads the `bans` array). There is no allowlist: a hit is a hit.
//
// Scope: walks src, tests, and examples only. Directories named scripts,
// archive, base-archive, and target are never entered, so the ban list itself
// and archived evidence stay out of the scan. Text-source extensions only.
//
// Modes: default scans the repository and exits 1 on any hit. Pass --selftest
// to prove the mechanism instead: a fixture carrying a banned literal must
// fail, a clean fixture must pass, and a skipped directory must be ignored.
// The gate counts the printed PASS/FAIL lines.

import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const BANS_FILE = join(HERE, "machine-path-bans.json");

const SCAN_DIRS = ["src", "tests", "examples"];
const SKIPPED_DIRS = new Set(["scripts", "archive", "base-archive", "target"]);
const SCAN_EXTENSIONS = new Set([
  "rs",
  "js",
  "mjs",
  "css",
  "html",
  "md",
  "yaml",
  "yml",
  "ps1",
  "json",
  "toml",
]);

function loadTestSourceBans() {
  const bans = JSON.parse(readFileSync(BANS_FILE, "utf8")).testSourceBans;
  if (!Array.isArray(bans) || bans.length === 0) {
    throw new Error(`${BANS_FILE} carries no testSourceBans array`);
  }
  return bans.map((pattern) => ({ pattern, re: new RegExp(pattern, "i") }));
}

function collectFiles(dir, out) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!SKIPPED_DIRS.has(entry.name)) collectFiles(join(dir, entry.name), out);
    } else if (entry.isFile()) {
      const dot = entry.name.lastIndexOf(".");
      const ext = dot < 0 ? "" : entry.name.slice(dot + 1).toLowerCase();
      if (SCAN_EXTENSIONS.has(ext)) out.push(join(dir, entry.name));
    }
  }
  return out;
}

function scanTree(root) {
  const bans = loadTestSourceBans();
  const hits = [];
  let files = 0;
  for (const dir of SCAN_DIRS) {
    const abs = join(root, dir);
    if (!existsSync(abs)) continue;
    for (const file of collectFiles(abs, [])) {
      files++;
      const relFile = relative(root, file).split(sep).join("/");
      const lines = readFileSync(file, "utf8").split(/\r?\n/);
      for (let index = 0; index < lines.length; index++) {
        for (const { pattern, re } of bans) {
          if (re.test(lines[index])) {
            hits.push({
              file: relFile,
              line: index + 1,
              pattern,
              excerpt: lines[index].trim().slice(0, 120),
            });
            break;
          }
        }
      }
    }
  }
  return { files, patterns: bans.length, hits };
}

// Proves the two contract legs without touching a repository file: the dirty
// fixture (a banned F: model literal) must produce exactly one hit on its own
// line, and the clean fixture tree must produce none. The target/ fixture
// proves the skip list is wired: a banned literal there must stay invisible.
function runSelftest() {
  const root = mkdtempSync(join(tmpdir(), "check-test-paths-selftest-"));
  const failures = [];
  try {
    mkdirSync(join(root, "src"), { recursive: true });
    mkdirSync(join(root, "tests"), { recursive: true });
    mkdirSync(join(root, "target"), { recursive: true });
    const dirtyPath = join(root, "tests", "dirty.rs");
    writeFileSync(dirtyPath, 'const P: &str = "F:/MODELS/x.gguf";\n');
    writeFileSync(join(root, "src", "clean.rs"), 'const P: &str = "models/x.gguf";\n');
    writeFileSync(join(root, "target", "skipped.rs"), 'const P: &str = "F:/MODELS/y.gguf";\n');

    const dirty = scanTree(root);
    const flagged =
      dirty.hits.length === 1 &&
      dirty.hits[0].file === "tests/dirty.rs" &&
      dirty.hits[0].line === 1;
    if (flagged) {
      console.log("PASS selftest: dirty fixture flagged (1 hit at tests/dirty.rs:1)");
    } else {
      failures.push("dirty fixture not flagged as expected");
      console.log(`FAIL selftest: dirty fixture produced ${dirty.hits.length} hit(s): ` +
        dirty.hits.map((h) => `${h.file}:${h.line}`).join(", "));
    }

    rmSync(dirtyPath, { force: true });
    const clean = scanTree(root);
    if (clean.hits.length === 0) {
      console.log("PASS selftest: clean fixture passed (0 hits, skipped dirs inert)");
    } else {
      failures.push("clean fixture not clean");
      console.log(
        `FAIL selftest: clean fixture produced ${clean.hits.length} hit(s): ` +
          clean.hits.map((h) => `${h.file}:${h.line}`).join(", ")
      );
    }
  } catch (error) {
    failures.push(String(error.message ?? error));
    console.log(`FAIL selftest: ${error.message ?? error}`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
  if (failures.length > 0) {
    console.log(`FAIL check-test-paths --selftest: ${failures.length} leg(s) failed`);
    process.exit(1);
  }
  console.log("PASS check-test-paths --selftest: all legs passed");
  process.exit(0);
}

if (process.argv.slice(2).includes("--selftest")) {
  runSelftest();
}

let result;
try {
  result = scanTree(ROOT);
} catch (error) {
  console.log(`FAIL check-test-paths: ${error.message ?? error}`);
  process.exit(1);
}

for (const hit of result.hits) {
  console.log(`FAIL ${hit.file}:${hit.line}: [${hit.pattern}] ${hit.excerpt}`);
}
if (result.hits.length > 0) {
  console.log(
    `FAIL check-test-paths: ${result.hits.length} banned literal(s) in ` +
      `${result.files} scanned file(s), ${result.patterns} pattern(s), no allowlist`
  );
  process.exit(1);
}
console.log(
  `PASS check-test-paths: ${result.files} file(s) scanned, 0 banned literals ` +
    `(${result.patterns} patterns)`
);
process.exit(0);
