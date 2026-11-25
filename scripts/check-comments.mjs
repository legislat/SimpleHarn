// Comment-hygiene checker for Rust comments in src/. Node only, no external
// dependencies. A source comment states the invariant and the reason; the
// durable ledger owns the history. This checker flags internal-history
// references inside line and block comments: task IDs, finding IDs, rule IDs
// (short "R3" handles and composites), review references ("review 25 [4]",
// "review W2 step 1"), private-layer paths, dated ruling cites
// ("operator GO 2026-11-01"), ledger finding IDs ("FINDINGS-12", "W21-01",
// "FR-01"), plan and evidence handles ("chunk 10 slice 10b", "C1 evidence"),
// release-version pins, queue task IDs ("T10.1"), development-phase handles
// ("phase 4"), matrix-cell handles ("Cell 4", "10a"), research-document cites
// ("Search#40", "research/x.md"), handoff-snapshot cites, and incident
// write-up names.
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
  { name: "finding-ledger", re: /\bFINDINGS-\d+\b/ },
  // Ledger variants: FINDINGS2-0910, FINDINGS-BATCH-2026-10-28B, REV9-01.
  { name: "finding-ledger-variant", re: /\b(?:FINDINGS\d+-|FINDINGS-BATCH-|REV\d+-)\d/ },
  // A review citation is "review <n>" with at most three digits, so a dated
  // cite like "review 2026-10-26" (a four-digit year) is not a review number.
  { name: "review-ref", re: /\breview\s+\d{1,3}\b/i },
  { name: "rule-id", re: /\bR\d+-\d+\b/ },
  // Short rule handles ("R3", "R11"): a bare ledger handle is not a public
  // anchor, so the comment must carry the invariant itself. The form never
  // occurs in a public token standard or identifier.
  { name: "rule-handle", re: /\bR\d+\b/ },
  // Review-wave handles ("W21-01", "W23-02") and wave step cites
  // ("review W2 step 1"). The wave numbering lives in the private ledger.
  { name: "wave-ref", re: /\bW\d+(?:-\d+|\s+step\s+\d+)\b/ },
  // Review-cycle finding ledgers. None of these families is anchored in a
  // public document: FR (1.4 findings), RC (readiness/crash), GSC, EVID
  // (card-run evidence IDs, private per PUBLICATION.md), PH-* (posture
  // findings), BS.
  { name: "fr-id", re: /\bFR-\d+\b/ },
  { name: "rc-id", re: /\bRC-\d+\b/ },
  { name: "gsc-id", re: /\bGSC-\d+\b/ },
  { name: "evid-id", re: /\bEVID-\d+\b/ },
  { name: "ph-id", re: /\bPH-[A-Z]+-\d+\b/ },
  { name: "bs-id", re: /\bBS-\d+\b/ },
  // A finding cite with an option letter, "F1(b)".
  { name: "finding-option", re: /\bF\d+\([a-z]\)/ },
  // A DECISIONS row cited as an identifier ("Row-10 guard"). The prose form
  // "row 19" reads as a cite to the public ledger and stays.
  { name: "row-handle", re: /\b[Rr]ow-\d+\b/ },
  // Dated ruling cites in both word orders: "operator GO 2026-11-01",
  // "RULED 2026-11-01", "operator decision 2026-10-25", "the 2026-11-04
  // ruling", "the 2026-11-15 gate ruling". A ruling is found in the public
  // ledger by its content, not by a date stamped into a source comment.
  { name: "dated-ruling-cite", re: /\b(?:OPERATOR|RULED|RULE|LOCKED|APPROVED|GO|DECISION)\s+20\d\d-\d\d\b|\b20\d\d-\d\d-\d\d\s+(?:[a-z-]+ )?(?:RULING|DECISION)\b/i },
  // The private orchestration layer ("ORCHESTRATION §1").
  { name: "orchestration-ref", re: /\bORCHESTRATION\b/ },
  // Review-wave prose: "green Wave C", "Wave-C", "review-wave leftover".
  { name: "review-wave", re: /\b(?:[Ww]ave[- ][A-Z]|review-wave)\b/ },
  // Plan chunk/slice handles: "chunk 10 slice 10b", "Slice 10c".
  { name: "chunk-slice", re: /\b(?:[Cc]hunk|[Ss]lice) \d+[a-z]\b/ },
  // Plan-item approval tags: "item 6 approved".
  { name: "item-approved", re: /\bitem \d+,? approved\b/i },
  // Release-cycle review cites: "the two 1.4 findings".
  { name: "review-cycle", re: /\b\d\.\d findings\b/ },
  // Card-run evidence cites: "card runs 2026-11-02", "the C1 evidence row".
  { name: "card-run-ref", re: /\b(?:card runs? 20\d\d-|C\d evidence)\b/i },
  // Explicit cites to the historical DESIGN plan ("DESIGN Step 4",
  // "DESIGN.md G1", "DESIGN §0.2"). DESIGN.md is historical rationale, never
  // a current spec, so it is not a public anchor for a source comment. A
  // bare "§5" without the DESIGN keyword is ambiguous and stays out of scope.
  { name: "design-step-ref", re: /\bDESIGN(?:\.md)?(?:\s*§|\s+Step\s+\d+)\b|\bDESIGN\.md\b/ },
  // Task IDs from the private queue ("T10.1", "T6-d"). The queue is not in the
  // public clone, so a task ID carries no meaning for a reader of the code.
  { name: "task-ref", re: /\bT\d+(?:\.\d+|-[a-z])\b/ },
  // Plan chunk handles with any digit form ("chunk 10 [4]", "Chunk-10"), and
  // development-phase handles ("phase 4", "Phase-4 pin").
  { name: "chunk-ref", re: /\b[Cc]hunk[- ]\d+\b/ },
  { name: "phase-ref", re: /\b[Pp]hase[- ]\d+\b/ },
  // Research-document cites from the private layer ("Search#40", "research/x.md").
  { name: "search-ref", re: /\b[Ss]earch\s*#\d+/ },
  { name: "research-ref", re: /\bresearch\/[\w-]+\.md\b/ },
  // The private handoff snapshot is not part of the public clone.
  { name: "handoff-doc", re: /\bHANDOFF\b/ },
  // Matrix-cell and finding-number handles ("Cell 4 option B", "park matrix",
  // "10a", "finding #1", "Axis 1").
  { name: "cell-ref", re: /\b(?:[Cc]ell\s*\d|matrix cell|park matrix|10a|finding\s*#\d+|[Aa]xis\s*[12])\b/ },
  // Dated ruling cites with the word before the date, any member of the
  // family ("ruling 2026-11-03", "amendment 2026-10-25", "ruling (2026-11-03)").
  // A DECISIONS row cited with its date names a public ledger entry and stays.
  { name: "ruling-date-suffix", re: /\b(?!(?:DECISIONS\b))(?:rulings?|rules?|amendments?|gates?|decisions?)\s*\(?\s*20\d\d-\d\d-\d\d\b/i },
  // Named incident write-ups from the private layer.
  { name: "incident-ref", re: /\bNINFER-\d+-INCIDENT\b/ },
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
