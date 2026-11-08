// Comment-hygiene checker for Rust comments in src/. Node only, no external
// dependencies. A source comment states the invariant and the reason; the
// durable ledger owns the history. This checker flags internal-history
// references inside line and block comments: task IDs, finding IDs, rule IDs,
// private-layer paths, ruling dates, and release-version pins.
//
// Modes: report mode is the default and always exits 0 with a per-file count;
// pass -Fail to exit 1 while any non-allowlisted hit remains.
//
// Allowlist: scripts/comment-allowlist.txt. Blank lines and lines beginning
// with '#' are ignored. Each other line is "<relative-path> <substring>" and
// suppresses hits on that file whose comment line contains the substring.
// Each entry states why the exception is deliberate.

import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { dirname, join, resolve, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const SRC = join(ROOT, "src");
const ALLOWLIST = join(HERE, "comment-allowlist.txt");

const FAIL_MODE = process.argv.slice(2).some((a) => /^(-{1,2}fail)$/i.test(a));

const PATTERNS = [
  { name: "task-id", re: /TASK-[A-Z]/ },
  { name: "finding-id", re: /\bF-\d+\b/ },
  { name: "rule-id", re: /\bR\d+-\d+\b/ },
  { name: "handoff-path", re: /handoff\//i },
  { name: "ruling-date", re: /ruling\s+20\d\d-/i },
  // Release pins look like "1.2.3". Four-octet forms (0.0.0.0, 127.0.0.1)
  // are addresses, not versions, so a trailing or leading octet excludes them.
  { name: "version-pin", re: /(?<![\d.])\d+\.\d+\.\d+(?!\.\d)/ },
];

function listRustFiles(dir, out) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) listRustFiles(p, out);
    else if (name.endsWith(".rs")) out.push(p);
  }
  return out;
}

// Returns Map<lineNumber, commentTextOnThatLine> for line ("//", "///", "//!")
// and block ("/* */", nested) comments, skipping string literals, raw strings
// (r"", r#".."#, b"", br#""#), and char literals, so URLs and regexes inside
// strings are never read as comments.
function extractComments(text) {
  const comments = new Map();
  let i = 0;
  let line = 1;
  const n = text.length;
  const put = (ch) => {
    comments.set(line, (comments.get(line) ?? "") + ch);
  };
  const skipRaw = (r) => {
    // text[r] === 'r'; returns index just past the closing delimiter, or -1.
    let j = r + 1;
    let hashes = 0;
    while (text[j] === "#") {
      hashes++;
      j++;
    }
    if (text[j] !== '"') return -1;
    j++;
    const term = '"' + "#".repeat(hashes);
    while (j < n) {
      if (text.startsWith(term, j)) return j + term.length;
      if (text[j] === "\n") line++;
      j++;
    }
    return n;
  };

  while (i < n) {
    const c = text[i];
    const d = text[i + 1];

    if (c === "/" && d === "/") {
      i += 2;
      while (i < n && text[i] !== "\n") {
        put(text[i]);
        i++;
      }
      continue;
    }
    if (c === "/" && d === "*") {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        if (text[i] === "/" && text[i + 1] === "*") {
          depth++;
        } else if (text[i] === "*" && text[i + 1] === "/") {
          depth--;
          i++;
          i++;
          continue;
        }
        if (text[i] === "\n") line++;
        else if (depth > 0) put(text[i]);
        i++;
      }
      continue;
    }

    let s = i;
    if (c === "b" && (d === '"' || d === "r")) s = i + 1;
    const sc = text[s];
    if (sc === "r") {
      const end = skipRaw(s);
      if (end >= 0) {
        i = end;
        continue;
      }
    }
    if (sc === '"') {
      let j = s + 1;
      while (j < n) {
        if (text[j] === "\\") {
          if (text[j + 1] === "\n") line++;
          j += 2;
          continue;
        }
        if (text[j] === '"') break;
        if (text[j] === "\n") line++;
        j++;
      }
      i = j + 1;
      continue;
    }

    if (c === "'" || (c === "b" && d === "'")) {
      const q = c === "b" ? i + 1 : i;
      if (text[q + 1] === "\\") {
        let j = q + 2;
        while (j < n && text[j] !== "'") j++;
        i = j + 1;
        continue;
      }
      if (text[q + 2] === "'") {
        i = q + 3;
        continue;
      }
      i++;
      continue;
    }

    if (c === "\n") line++;
    i++;
  }
  return comments;
}

function loadAllowlist() {
  const entries = [];
  if (!existsSync(ALLOWLIST)) return entries;
  for (const raw of readFileSync(ALLOWLIST, "utf8").split(/\r?\n/)) {
    const s = raw.trim();
    if (s === "" || s.startsWith("#")) continue;
    const sp = s.indexOf(" ");
    if (sp < 0) continue;
    entries.push({ file: s.slice(0, sp), needle: s.slice(sp + 1) });
  }
  return entries;
}

const rel = (p) => relative(ROOT, p).split(sep).join("/");
const allowlist = loadAllowlist();

let files;
try {
  files = listRustFiles(SRC, []);
} catch {
  console.log("check-comments: src/ not found; nothing to check");
  process.exit(0);
}

const hits = [];
let allowed = 0;
for (const file of files) {
  const relFile = rel(file);
  const comments = extractComments(readFileSync(file, "utf8"));
  for (const [lineNum, text] of comments) {
    for (const { name, re } of PATTERNS) {
      if (!re.test(text)) continue;
      const entry = allowlist.find(
        (a) => a.file === relFile && text.includes(a.needle),
      );
      if (entry) {
        allowed++;
        break;
      }
      hits.push({ file: relFile, line: lineNum, pattern: name, text: text.trim() });
      break;
    }
  }
}

const byFile = new Map();
for (const h of hits) byFile.set(h.file, (byFile.get(h.file) ?? 0) + 1);

for (const h of hits) {
  console.log(`${h.file}:${h.line}: [${h.pattern}] ${h.text}`);
}
for (const [file, count] of [...byFile].sort((a, b) => b[1] - a[1])) {
  console.log(`COUNT ${file}: ${count}`);
}

const summary = `check-comments: ${hits.length} hit(s) in ${byFile.size} file(s)` +
  (allowed > 0 ? `, ${allowed} allowlisted` : "") +
  (FAIL_MODE ? "" : " (report mode)");
console.log(summary);

if (FAIL_MODE && hits.length > 0) {
  console.log("check-comments: FAIL");
  process.exit(1);
}
console.log("check-comments: PASS");
process.exit(0);
