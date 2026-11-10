// Release-facts checker. Node ESM, built-in modules only, no network.
// Two release facts, derived once and enforced everywhere:
//   - the crate version: the three-part semver in Cargo.toml. The webui
//     fixture and every version comparison use it, always three-part.
//   - the release tag: the first (latest) dated CHANGELOG.md heading, which
//     may carry a fourth hotfix segment (v1.5.1.1, docs/process/GIT_WORKFLOW.md
//     "Tag and version naming"). Its first three segments must equal the
//     crate version. Headings marked "unreleased" name the next release under
//     preparation; they drive no current claim.
// The documentation-review findings F-02 and F-05 showed that shape/link
// checks pass while the content is wrong, so this checker pins semantic
// release facts:
//   1. CHANGELOG.md: a latest dated release heading exists, and its first
//      three segments equal the crate version; that heading is the tag.
//   2. README.md: "Current release: **<tag>**." is present.
//   3. README.md: the quick start names the asset
//      SimpleHarn-<tag-without-v>-win-x64.zip and the Linux download line
//      names SimpleHarn-<tag-without-v>-linux-x64.tar.gz.
//   4. README.md: the "The release zip holds" paragraph names exactly the
//      five canonical package entries as backticked items (any order).
//   5. docs/reference/COMPATIBILITY.md: "Current release: **<tag>**." is
//      present and the stale release claims are gone.
//   6. tests/webui/fixtures/status.json: the top-level "version" equals the
//      three-part crate version.
//   7. README.md: the Plans section does not list a shipped capability
//      (Linux ships in the current release).
//   8. The linux-artifact job pins ubuntu-24.04 and an explicit base image,
//      records GLIBC_FLOOR, and README Requirements and COMPATIBILITY name
//      that same floor.
//   9. scripts/make-public-asset.ps1: the zip name is built as
//      SimpleHarn-<version-without-v>-win-x64.zip and the five canonical
//      entries appear.
// Usage: node scripts/check-doc-facts.mjs [--root <dir>]
// Exit 0 with "check-doc-facts: PASS (n checks)" or exit 1 naming every FAIL.

import { readFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

function parseRoot(argv) {
  const i = argv.indexOf("--root");
  if (i >= 0) {
    const value = argv[i + 1];
    if (!value || value.startsWith("--")) {
      console.error("check-doc-facts: --root needs a directory argument");
      process.exit(2);
    }
    return resolve(value);
  }
  return resolve(HERE, "..");
}

const ROOT = parseRoot(process.argv.slice(2));

const CANONICAL_PACKAGE = [
  "simpleharn.exe",
  "SimpleHarn.vbs",
  "README.md",
  "LICENSE",
  "docs/assets/control-page.png",
];
// Phrases review F-03 proved false for the current release.
const STALE_COMPAT_PHRASES = [
  "ship in no released binary",
  "v1.3.0, the current release",
  "main (unreleased)",
];

const failures = [];
let checks = 0;

function fail(file, expected, actual) {
  failures.push(`FAIL ${file}: expected ${expected}, actual ${actual}`);
}

function check(condition, file, expected, actual) {
  checks += 1;
  if (!condition) fail(file, expected, actual);
}

function read(rel) {
  const abs = join(ROOT, rel);
  if (!existsSync(abs)) return null;
  return readFileSync(abs, "utf8");
}

function present(rel) {
  checks += 1;
  if (read(rel) === null) {
    fail(rel, "file to exist", "file not found under root " + ROOT);
    return null;
  }
  return read(rel);
}

const normalize = (s) => s.replace(/\s+/g, " ").trim();
const normalizePaths = (s) => normalize(s).replace(/\\/g, "/");

// 0. The version: Cargo.toml is the single truth for every fact below.
const cargo = present("Cargo.toml");
let version = null;
if (cargo !== null) {
  const m = cargo.match(/^\s*version\s*=\s*"([^"]+)"/m);
  version = m ? m[1] : null;
  check(m !== null, "Cargo.toml", 'a package version line: version = "X.Y.Z"', "no match");
}

if (version !== null) {
  // The release tag: the first (latest) DATED CHANGELOG.md heading. It may
  // carry the optional fourth hotfix segment (docs/process/GIT_WORKFLOW.md,
  // "Tag and version naming"), so it is derived here, not built from the
  // crate version. Headings marked "unreleased" name the next release under
  // preparation and drive no current claim. The fixture and Cargo
  // comparisons below keep using the three-part crate version.
  const changelog = present("CHANGELOG.md");
  let tag = null;
  if (changelog !== null) {
    const heads = [];
    for (const line of changelog.split(/\r?\n/)) {
      const m = line.match(/^## (v\d+\.\d+\.\d+(?:\.\d+)?)(.*)$/);
      if (m) heads.push({ tag: m[1], rest: m[2] });
    }
    const unreleased = (h) => /\bunreleased\b/.test(h.rest);
    const latest = heads.find((h) => !unreleased(h)) ?? null;
    check(
      latest !== null,
      "CHANGELOG.md",
      "a latest dated release heading",
      "no dated release heading"
    );
    if (latest !== null) {
      tag = latest.tag;
      // Hotfix rule: the tag's first three segments must name the crate version.
      check(
        tag.slice(1).split(".").slice(0, 3).join(".") === version,
        "CHANGELOG.md",
        `latest release heading "${latest.tag}" whose first three segments equal the crate version ${version}`,
        `"${latest.tag}" names a different core`
      );
    }
    const prepared = heads.find(unreleased) ?? null;
    if (prepared !== null) {
      check(
        prepared.tag.slice(1).split(".").slice(0, 3).join(".") === version,
        "CHANGELOG.md",
        `unreleased heading "${prepared.tag}" whose first three segments equal the crate version ${version}`,
        `"${prepared.tag}" names a different core`
      );
    }
  }
  if (tag === null) tag = "v" + version;
  const tagVersion = tag.replace(/^v/, "");

  // 2..4 README.md release line, quick-start asset, package sentence.
  const readme = present("README.md");
  if (readme !== null) {
    const releaseLine = `Current release: **${tag}**.`;
    check(
      readme.includes(releaseLine),
      "README.md",
      `the sentence "${releaseLine}"`,
      readme.includes("Current release:")
        ? "a different Current release line"
        : "no Current release line"
    );
    const asset = `SimpleHarn-${tagVersion}-win-x64.zip`;
    check(
      readme.includes(asset),
      "README.md",
      `the quick-start asset name "${asset}"`,
      "the asset name is absent or names a different version"
    );
    const linuxAsset = `SimpleHarn-${tagVersion}-linux-x64.tar.gz`;
    check(
      readme.includes(linuxAsset),
      "README.md",
      `the Linux download asset name "${linuxAsset}"`,
      "the Linux asset name is absent or names a different version"
    );
    const anchor = "The release zip holds";
    const flat = normalize(readme);
    const at = flat.indexOf(anchor);
    if (at < 0) {
      checks += 1;
      fail("README.md", `the sentence starting "${anchor}"`, "sentence not found");
    } else {
      // Match inside the whitespace-normalized paragraph that holds the
      // anchor, so a line-wrapped sentence still parses.
      const paragraphs = readme.split(/\r?\n\s*\r?\n/);
      const paragraph = paragraphs.find((p) => normalize(p).includes(anchor)) ?? "";
      const sentence = normalize(paragraph);
      const named = [...sentence.matchAll(/`([^`]+)`/g)]
        .map((m) => m[1].replace(/\\/g, "/"))
        .sort();
      const canonical = [...CANONICAL_PACKAGE].sort();
      check(
        JSON.stringify(named) === JSON.stringify(canonical),
        "README.md",
        `the package paragraph to name exactly ${canonical.length} entries: ${canonical.join(", ")}`,
        named.length === 0 ? "no backticked entry in the paragraph" : `named: ${named.join(", ")}`
      );
    }
  }

  // 5. COMPATIBILITY.md: current release claim, no stale phrases.
  const compat = present("docs/reference/COMPATIBILITY.md");
  if (compat !== null) {
    const releaseLine = `Current release: **${tag}**.`;
    check(
      compat.includes(releaseLine),
      "docs/reference/COMPATIBILITY.md",
      `the sentence "${releaseLine}"`,
      "sentence absent"
    );
    for (const phrase of STALE_COMPAT_PHRASES) {
      check(
        !compat.includes(phrase),
        "docs/reference/COMPATIBILITY.md",
        `no stale phrase "${phrase}"`,
        "phrase still present"
      );
    }
  }

  // 6. Web UI fixture: top-level "version" equals the crate version.
  const fixture = present("tests/webui/fixtures/status.json");
  if (fixture !== null) {
    let fixtureVersion = null;
    let parsed = null;
    try {
      parsed = JSON.parse(fixture);
    } catch {
      checks += 1;
      fail("tests/webui/fixtures/status.json", "valid JSON", "parse error");
    }
    if (parsed !== null) {
      fixtureVersion = parsed.version ?? null;
      check(
        fixtureVersion === version,
        "tests/webui/fixtures/status.json",
        `top-level "version" equal to "${version}"`,
        String(fixtureVersion)
      );
    }
  }

  // 7. README Plans: a shipped capability must not still be listed as a plan.
  // Linux shipped in 1.5.0 and the shipped artifact ships the same zip name;
  // the Plans section stayed stale through two releases before a reader
  // reported it, so the absence is now checked.
  if (readme !== null) {
    const plans = readme.split(/^## Plans\s*$/m)[1]?.split(/^## /m)[0] ?? "";
    check(
      plans.length > 0 && !/linux/i.test(plans),
      "README.md",
      "a Plans section that does not list Linux, which ships in the current release",
      plans.length === 0 ? "no Plans section" : "Plans still names Linux"
    );
  }

  // 8. Linux artifact floor (RELEASE-LINUX-GLIBC): the linux-artifact job
  // pins the build base image and records the measured glibc floor in
  // GLIBC_FLOOR; README Requirements and COMPATIBILITY must state the same
  // floor, and the job must stay pinned to explicit images, never a floating
  // runner or an unpinned base. The v1.5.1 artifact required GLIBC_2.39
  // while no doc named a floor, so a first-time Debian 12 or RHEL 9 user met
  // a raw loader error; this check ties the docs and the workflow together.
  const facts = present(".github/workflows/release-facts.yml");
  if (facts !== null) {
    const job = facts.split(/^  linux-artifact:/m)[1]?.split(/^  [a-z-]+:/m)[0] ?? "";
    check(job.length > 0, ".github/workflows/release-facts.yml",
      "a linux-artifact job", "no linux-artifact job");
    const fm = job.match(/GLIBC_FLOOR:\s*'?(\d+\.\d+)'?/);
    check(fm !== null, ".github/workflows/release-facts.yml",
      "a GLIBC_FLOOR env value naming the measured glibc floor",
      job.length === 0 ? "no linux-artifact job" : "no GLIBC_FLOOR value");
    if (fm !== null) {
      const floor = fm[1];
      const floorRe = new RegExp(`glibc\\s+${floor.replaceAll(".", "\\.")}\\b`);
      // Scope to the Requirements section with the same split-and-cut idiom
      // the Plans fact uses: the label claims the Requirements paragraph, so
      // an intro mention elsewhere must not stand in for it.
      const requirements =
        (readme ?? "").split(/^## Requirements\s*$/m)[1]?.split(/^## /m)[0] ?? "";
      check(requirements.length > 0 && floorRe.test(requirements), "README.md",
        "the Requirements paragraph names the glibc " + floor +
        " floor of the prebuilt Linux artifact",
        requirements.length === 0
          ? "no Requirements section"
          : "the floor is absent from Requirements or names a different version");
      check(compat !== null && floorRe.test(compat), "docs/reference/COMPATIBILITY.md",
        "the artifact paragraph names the same glibc " + floor + " floor",
        "the floor is absent or names a different version");
      // A moved floor must scrub the old claim: the recorded past floors
      // fail as stale while the workflow names a different one. 2.39 was the
      // v1.5.1 claim built on ubuntu-24.04.
      for (const staleFloor of ["2.39"]) {
        if (staleFloor === floor) continue;
        const stale = "glibc " + staleFloor;
        check(!(readme ?? "").includes(stale), "README.md",
          `no stale "${stale}" claim`, "stale claim present");
        check(!(compat ?? "").includes(stale), "docs/reference/COMPATIBILITY.md",
          `no stale "${stale}" claim`, "stale claim present");
      }
    }
    check(
      /runs-on:\s+ubuntu-24\.04\b/.test(job) && !/runs-on:\s+ubuntu-latest\b/.test(job),
      ".github/workflows/release-facts.yml",
      "a linux-artifact host pinned to ubuntu-24.04, never ubuntu-latest",
      job.length === 0 ? "no linux-artifact job" : "the job host uses a floating runner image"
    );
    const img = job.match(/ARTIFACT_BASE:\s*(\S+)/);
    check(
      img !== null && /:/.test(img[1]) && !img[1].endsWith(":latest"),
      ".github/workflows/release-facts.yml",
      "the linux-artifact build base pinned to an explicit image tag (ARTIFACT_BASE, never :latest)",
      img === null ? "no ARTIFACT_BASE value" : "unpinned base image " + img[1]
    );
  }

  // 9. make-public-asset.ps1: zip name pattern and package manifest.
  const maker = present("scripts/make-public-asset.ps1");
  if (maker !== null) {
    const direct = /SimpleHarn-"\s*\+\s*\$Version\b/;
    const built = /SimpleHarn-"\s*\+\s*\$[A-Za-z]\w*/;
    const vstrip = /\$Version\s+-replace\s+'\^v'/;
    check(
      maker.includes('"-win-x64.zip"') && built.test(maker) && vstrip.test(maker) && !direct.test(maker),
      "scripts/make-public-asset.ps1",
      'a zip name built as "SimpleHarn-" + (version without v) + "-win-x64.zip"',
      direct.test(maker)
        ? "the zip name still embeds the tag variable directly (keeps the v)"
        : "zip name pattern not found"
    );
    const flat = normalizePaths(maker);
    for (const entry of CANONICAL_PACKAGE) {
      check(
        flat.includes(entry),
        "scripts/make-public-asset.ps1",
        `the canonical package entry "${entry}"`,
        "entry not found in the script"
      );
    }
  }
}

if (failures.length === 0) {
  console.log(`check-doc-facts: PASS (${checks} checks)`);
  process.exit(0);
}
for (const line of failures) console.log(line);
console.log(`check-doc-facts: FAIL (${failures.length} of ${checks} checks failed)`);
process.exit(1);
