// Documentation checker for durable docs. Node, no external dependencies.
// Rules (docs/DOCS_README.md sections 5 and 6):
//   1. Front matter with the seven required keys. README.md is exempt: it is the
//      storefront page and GitHub renders front matter there as a table (2026-11-05 ruling).
//   2. Relative Markdown links resolve to existing files, and same-page anchors
//      (#name) resolve to a heading whose GitHub slug matches (duplicate headings
//      take -1, -2 suffixes). Cross-file anchors are not checked.
//   3. No author-specific absolute Windows drive paths in durable docs. Documented
//      product defaults in README/reference and exact quoted decisions are allowed.
//      (fail inside code fences too, except for those documented defaults). The
//      shared machine-path ban set in scripts/machine-path-bans.json is enforced
//      here and by scripts/release-check.ps1 alike.
//   4. No duplicate H1.
//   5. Every durable doc is listed in docs/DOCS_README.md by exact path or by a
//      link that resolves to it.
//   6. No literal DRAFT in a doc whose front matter status is active.
//   7. Repo-relative paths written as inline code or plain prose resolve to
//      existing files. Local-only paths (handoff/, ModelConf/, archive/,
//      base-archive/, .kilo/) are exempt; a path that names a line range keeps
//      its base file. The prose pass is shape-conservative: a letter-leading
//      first segment, no spaces, and an extension or a tracked top-level
//      directory; dated evidence-run names and domains are not repo paths.
//      The historical DESIGN.md is exempt from this rule only: its dated
//      research references are frozen history.
//   8. Semantic cross-check: every measured_attribution wire variant parsed by
//      src/model/mod.rs (the BadAttribution error string) is listed on the
//      measured_attribution line of docs/reference/API.md.
// Not scanned: handoff/* (local), archive/ and base-archive/ (frozen). The
// frozen pages inside docs/archive/ are not scanned either; only its README,
// the archive index itself, is a durable checked document.

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const INDEX = "docs/DOCS_README.md";
const REQUIRED_KEYS = [
  "title", "owner", "status", "audience",
  "source_of_truth_for", "last_reviewed", "review_trigger",
];
const FRONT_MATTER_EXEMPT = new Set(["README.md"]);

const failures = [];
const fail = (path, line, message) => failures.push(`${path}:${line}: ${message}`);

// Exact quoted rulings may contain the operator's paths: the two ledgers are
// exempt from the generic drive rule. The shared machine-path bans below apply
// to every durable doc, exactly as scripts/release-check.ps1 applies them, so
// one machine path can never pass this gate and fail the release check.
const PATH_EXEMPT = new Set([
  "docs/architecture/DECISIONS.md",
  "docs/architecture/DOCTRINE.md",
]);
// The canonical machine-path ban set, read from the one shared file so this
// checker and scripts/release-check.ps1 can never disagree. The author-machine
// root is banned whole: the harness ships no default under it (FR-15).
const MACHINE_PATH_BANS = JSON.parse(
  readFileSync(join(HERE, "machine-path-bans.json"), "utf8")
).bans.map((pattern) => new RegExp(pattern, "i"));
// Documented product defaults: the strata install home and the model-root
// example, both named in README/reference. The match is boundary-anchored: the
// default name must end at a path boundary, so a name continuation (for
// example C:\Strata-nvfp4) is not a documented default and still fails.
// Author-specific paths (for example C:\Users\... or an F:\ model path) are
// not product defaults and still fail.
const PRODUCT_DEFAULT_PATH =
  /(?:C:\\Strata|D:\\Models)(?![A-Za-z0-9_.\-])/i;

function listFiles(relDir) {
  const abs = join(ROOT, relDir);
  if (!existsSync(abs)) return [];
  return readdirSync(abs)
    .filter((n) => n.endsWith(".md"))
    .sort()
    .map((n) => `${relDir}/${n}`.split(sep).join("/"));
}

const candidates = [
  "README.md", "AGENTS.md", "CONTRIBUTING.md", "CHANGELOG.md", "SECURITY.md",
  "DESIGN.md", INDEX,
  "docs/archive/README.md", "examples/README.md",
  ...listFiles("docs/process"),
  ...listFiles("docs/architecture"),
  ...listFiles("docs/evidence"),
  ...listFiles("docs/reference"),
];
const docs = candidates.filter((p) => existsSync(join(ROOT, p)));

// Split a document into lines with a flag for "inside a code fence".
function parseLines(text) {
  let inFence = false;
  return text.split(/\r?\n/).map((line, i) => {
    if (/^(```|~~~)/.test(line)) inFence = !inFence;
    return { line, n: i + 1, inFence };
  });
}

function parseFrontMatter(rel, entries) {
  if (!entries.length || entries[0].line.trim() !== "---") {
    fail(rel, 1, "missing front matter block");
    return null;
  }
  const keys = new Map();
  let status = "";
  for (const e of entries.slice(1, entries.length - 1)) {
    if (e.line.trim() === "---") break;
    const m = e.line.match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$/);
    if (m) {
      keys.set(m[1], m[2].trim());
      if (m[1] === "status") status = m[2].trim().toLowerCase();
    }
  }
  return { keys, status };
}

// GitHub heading slugs: lowercase, drop punctuation and symbols (keep word
// characters, spaces, and hyphens), then spaces to hyphens. Duplicate headings
// gain -1, -2 suffixes, as on GitHub.
function headingSlug(text) {
  return text
    .toLowerCase()
    .replace(/[\u2000-\u206F\u2E00-\u2E7F\\'!"#$%&()*+,./:;<=>?@[\]^`{|}~]/g, "")
    .replace(/\s/g, "-");
}

function headingSlugs(entries) {
  const slugs = new Set();
  const counts = new Map();
  for (const e of entries) {
    if (e.inFence) continue;
    const m = e.line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (!m) continue;
    const text = m[2].replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");
    let slug = headingSlug(text);
    const seen = counts.get(slug) || 0;
    if (seen > 0) slug = `${slug}-${seen}`;
    counts.set(headingSlug(text), seen + 1);
    slugs.add(slug);
  }
  return slugs;
}

function checkLinks(rel, entries, slugs) {
  const baseDir = dirname(join(ROOT, rel));
  for (const e of entries) {
    if (e.inFence) continue;
    const re = /\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g;
    let m;
    while ((m = re.exec(e.line)) !== null) {
      let target = m[1];
      if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue; // http:, mailto:, ...
      if (target.startsWith("#")) {
        const anchor = decodeURIComponent(target.slice(1)).toLowerCase();
        if (!slugs.has(anchor)) {
          fail(rel, e.n, `unresolved same-page anchor: ${target}`);
        }
        continue;
      }
      target = decodeURIComponent(target.split("#")[0]);
      if (target === "") continue;
      if (!existsSync(resolve(baseDir, target))) {
        fail(rel, e.n, `broken relative link: ${m[1]}`);
      }
    }
  }
}

// Rule 7: repo-relative paths in inline code. The set below is the documented
// local-only layer: its paths exist only on the operator machine.
const LOCAL_ONLY_PREFIXES = [
  "handoff/", "ModelConf/", "archive/", "base-archive/", ".kilo/",
  "target/", "node_modules/",
];
const LOCAL_ONLY_FILES = new Set(["HANDOFF.md"]);
const TRACKED_PREFIXES = [
  "docs/", "src/", "tests/", "scripts/", "assets/", "examples/", ".github/",
];
const HAS_EXTENSION = /\.[A-Za-z0-9]{1,6}$/;

function isLocalOnly(target) {
  return LOCAL_ONLY_FILES.has(target) ||
    LOCAL_ONLY_PREFIXES.some((p) => target.startsWith(p));
}

// Rule 7 exemption: the historical design record cites dated local research
// paths that no longer resolve, and its frozen text is never edited to satisfy
// a checker (the review's "stale local-only references are historical by
// design"). Every other rule still applies to it.
const PATH_RULE_EXEMPT = new Set(["DESIGN.md"]);

// Shared shape gate for rule 7. A candidate must be a simple repo-relative
// path: no spaces, no scheme, no drive or backslash, not absolute, not a flag,
// no glob or placeholder, at least one separator, an extension or a tracked
// top-level directory, and not under the exempt local-only layer.
function isPathShape(target) {
  if (target === "" || /\s/.test(target)) return false;
  if (/^[a-z][a-z0-9+.-]*:/i.test(target)) return false; // URL, scheme, or drive
  if (target.includes("\\")) return false; // absolute Windows path
  if (target.startsWith("/") || target.startsWith("-")) return false; // absolute or flag
  if (/[*?<>{}|$]/.test(target)) return false; // glob, placeholder, pipe
  if (!target.includes("/")) return false; // bare names and identifiers
  if (isLocalOnly(target)) return false;
  if (target.endsWith("/")) return false; // a directory reference (policy target)
  const stripped = target.replace(/:\d+(?:-\d+)?$/, ""); // file.rs:120-130
  if (!HAS_EXTENSION.test(stripped) &&
      !TRACKED_PREFIXES.some((p) => stripped.startsWith(p))) return false;
  return true;
}

function checkPathResolves(rel, n, display, target) {
  const stripped = target.replace(/:\d+(?:-\d+)?$/, "");
  const roots = [ROOT, join(ROOT, "src"), join(ROOT, "tests"), join(ROOT, "scripts")];
  if (!roots.some((root) => existsSync(resolve(root, stripped)))) {
    fail(rel, n, `unresolved path reference: ${display}`);
  }
}

// Prose-pass guard: the first segment must start with a letter. That keeps
// dated evidence-run names (for example 2026-11-09-linux-basalt/PROOF.md,
// which point at the private handoff evidence directory) and bare domains
// (quay.io/...) out of the rule. A real top-level repo directory always starts
// with a letter.
const LETTER_FIRST_SEGMENT = /^[A-Za-z][A-Za-z0-9_-]*\//;

function checkInlinePaths(rel, entries) {
  if (PATH_RULE_EXEMPT.has(rel)) return;
  for (const e of entries) {
    if (e.inFence) continue;
    // Inline code spans.
    const re = /`([^`\n]+)`/g;
    let m;
    while ((m = re.exec(e.line)) !== null) {
      const target = m[1].trim();
      if (!isPathShape(target)) continue;
      checkPathResolves(rel, e.n, m[1], target);
    }
    // Plain prose, outside code spans and link targets (rule 2 owns links).
    const prose = e.line
      .replace(/`[^`\n]*`/g, " ")
      .replace(/\]\([^)\s]*(?:\s+"[^"]*")?\)/g, " ");
    for (const raw of prose.split(/\s+/)) {
      const target = raw
        .replace(/^[(\[{<>"'`]+/, "")
        .replace(/[.,;:!?)\]}>;"'`]+$/, "");
      if (!isPathShape(target) || !LETTER_FIRST_SEGMENT.test(target)) continue;
      checkPathResolves(rel, e.n, target, target);
    }
  }
}

// Rule 8: the wire enum for measured_attribution. The parser refuses an
// unknown token with the BadAttribution error, whose string lists every
// variant the code accepts; the check takes that list as truth and demands
// each variant in the API.md field line.
function checkAttributionWire() {
  const apiRel = "docs/reference/API.md";
  const variants = wireAttributionVariants();
  if (!variants) {
    fail(apiRel, 1,
      "wire cross-check: cannot parse the measured_attribution variant list from the BadAttribution error string in src/model/mod.rs");
    return;
  }
  if (!existsSync(join(ROOT, apiRel))) {
    fail(apiRel, 1, "wire cross-check: docs/reference/API.md is missing");
    return;
  }
  const api = readFileSync(join(ROOT, apiRel), "utf8").split(/\r?\n/);
  const start = api.findIndex((l) => /^\s*-\s+`measured_attribution`:/.test(l));
  if (start < 0) {
    fail(apiRel, 1, "wire cross-check: no measured_attribution field line in docs/reference/API.md");
    return;
  }
  let block = api[start];
  for (let i = start + 1; i < api.length && /^\s{2,}\S/.test(api[i]); i += 1) {
    block += " " + api[i];
  }
  for (const v of variants) {
    if (!block.includes(v)) {
      fail(apiRel, start + 1,
        `wire cross-check: API.md measured_attribution list is missing the wire variant \`${v}\` (src/model/mod.rs parses ${variants.join("|")})`);
    }
  }
}

function wireAttributionVariants() {
  const srcRel = "src/model/mod.rs";
  if (!existsSync(join(ROOT, srcRel))) return null;
  const src = readFileSync(join(ROOT, srcRel), "utf8");
  const at = src.indexOf("BadAttribution");
  if (at < 0) return null;
  const errors = [...src.slice(0, at).matchAll(/#\[error\("([^"]+)"\)\]/g)];
  const last = errors[errors.length - 1];
  const list = last && last[1].match(/is not one of ([A-Za-z0-9_]+(?:\|[A-Za-z0-9_]+)+)/);
  return list ? list[1].split("|") : null;
}

const indexText = existsSync(join(ROOT, INDEX))
  ? readFileSync(join(ROOT, INDEX), "utf8")
  : "";
if (indexText === "") fail(INDEX, 1, "documentation index is missing; coverage cannot be checked");

// Exact index coverage: every link target in the index, resolved against the
// index file's directory. A plain full-path string in the index text also
// counts; a bare filename no longer does (substring matching hid orphans).
const indexResolved = new Set();
if (indexText !== "") {
  const indexDir = dirname(join(ROOT, INDEX));
  for (const e of parseLines(indexText)) {
    if (e.inFence) continue;
    const re = /\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g;
    let m;
    while ((m = re.exec(e.line)) !== null) {
      let target = m[1];
      if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue;
      target = decodeURIComponent(target.split("#")[0]);
      if (target === "") continue;
      const abs = resolve(indexDir, target);
      const relTarget = relative(ROOT, abs).split(sep).join("/");
      if (!relTarget.startsWith("..")) indexResolved.add(relTarget);
    }
  }
}

for (const rel of docs) {
  const text = readFileSync(join(ROOT, rel), "utf8");
  const entries = parseLines(text);

  // 1. Front matter. README.md is exempt (storefront page); its status is active.
  const exempt = FRONT_MATTER_EXEMPT.has(rel);
  const fm = exempt ? { keys: new Map(), status: "active" } : parseFrontMatter(rel, entries);
  if (exempt || fm) {
    if (!exempt) {
      for (const key of REQUIRED_KEYS) {
        if (!fm.keys.has(key)) fail(rel, 1, `front matter missing key: ${key}`);
      }
    }

    // 6. DRAFT ban on active docs (literal, case-sensitive, outside code fences).
    if (fm.status === "active") {
      for (const e of entries) {
        if (!e.inFence && e.line.includes("DRAFT")) {
          fail(rel, e.n, "literal DRAFT in a doc whose status is active");
        }
      }
    }
  }

  // 2. Relative links and same-page anchors.
  checkLinks(rel, entries, headingSlugs(entries));

  // 7. Repo-relative paths in inline code.
  checkInlinePaths(rel, entries);

  // 3. Machine paths, code fences included. The shared ban set first: every
  //    durable doc, ruling copies included, exactly like release-check.ps1.
  //    Then the generic drive rule: it rejects author-specific paths on any
  //    drive, exempts the exact ruling copies, and allows the documented
  //    product defaults.
  entries.forEach((e) => {
    for (const ban of MACHINE_PATH_BANS) {
      if (ban.test(e.line)) {
        fail(rel, e.n, `machine path in durable doc (shared ban ${ban.source})`);
        break;
      }
    }
  });
  if (!PATH_EXEMPT.has(rel)) {
    entries.forEach((e) => {
      const m = e.line.match(/(^|[^A-Za-z0-9])([A-Za-z]):[\\/]/);
      if (m && !PRODUCT_DEFAULT_PATH.test(e.line)) {
        fail(rel, e.n, `absolute Windows path in durable doc: ${m[2]}:\\...`);
      }
      const home = e.line.match(/\/(?:home|Users)\/[A-Za-z0-9._-]+\//);
      if (home && !/\/(?:home|Users)\/(?:you|user|username)\//.test(home[0])) {
        fail(rel, e.n, `author home path in durable doc: ${home[0]}`);
      }
    });
  }

  // 4. Single H1 outside code fences.
  const h1s = entries.filter((e) => !e.inFence && /^#\s+\S/.test(e.line));
  for (const e of h1s.slice(1)) {
    fail(rel, e.n, `duplicate H1 (first H1 is line ${h1s[0].n})`);
  }

  // 5. Index coverage: the index lists every durable doc by exact path, by a
  //    link that resolves to it, or by a docs-relative path string. The index
  //    itself cannot list itself.
  if (rel !== INDEX && indexText !== "") {
    const docsRel = rel.startsWith("docs/") ? rel.slice("docs/".length) : null;
    const listed = indexResolved.has(rel) || indexText.includes(rel) ||
      (docsRel !== null && indexText.includes(docsRel));
    if (!listed) {
      fail(rel, 1, `orphan doc: not listed in ${INDEX} by exact path or resolving link`);
    }
  }
}

checkAttributionWire();

for (const f of failures) console.log(f);
if (failures.length > 0) {
  console.log(`check-docs: FAIL (${failures.length})`);
  process.exit(1);
}
console.log(`check-docs: PASS (${docs.length} durable docs checked)`);
