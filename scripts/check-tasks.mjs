// Task file checker for the private handoff repository's tasks/*.md. Node, no
// external dependencies. Schema: docs/process/TASKS.md. The handoff root is
// resolved by scripts/lib/handoff-root.mjs (see that file for the order). An
// absent root or an empty task directory is valid: a public clone has no
// private layer, and the queue can legitimately hold no task files.
//
// Required per task file:
//   - an H1 beginning "# TASK-";
//   - fields: Class, Priority, Readiness, Owner (table rows) and Goal, Scope,
//     Non-goals, Acceptance criteria, Dependencies, Evidence, Next step
//     (table rows or section headings);
//   - Class in S/R/C/O, Priority in P0-P3, Readiness in
//     ready/blocked/needs-design/needs-operator.
//
// Grandfather list: tasks/.schema-grandfathered names the legacy files that
// predate this schema. The default run skips exactly the listed files; every
// unlisted file (every new task) is always validated, so the list cannot
// silently grow. An absent list skips nothing. A listed file that now conforms
// is warned about, never failed. Pass -Strict to ignore the list entirely and
// report every failure, which keeps the debt measurable.

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { handoffRoot } from "./lib/handoff-root.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const HANDOFF = handoffRoot(ROOT);
if (!HANDOFF) {
  console.log("check-tasks: SKIP - handoff root not found (set SIMPLEHARN_HANDOFF or clone the private repo beside this one)");
  process.exit(0);
}
const TASK_DIR = join(HANDOFF, "tasks");
const STRICT = process.argv.slice(2).includes("-Strict");

const TABLE_FIELDS = ["Class", "Priority", "Readiness", "Owner"];
const TEXT_FIELDS = ["Goal", "Scope", "Non-goals", "Acceptance criteria",
  "Dependencies", "Evidence", "Next step"];
const CLASS_VALUES = ["S", "R", "C", "O"];
const PRIORITY_VALUES = ["P0", "P1", "P2", "P3"];
const READINESS_VALUES = ["ready", "blocked", "needs-design", "needs-operator"];

if (!existsSync(TASK_DIR)) {
  console.log(`check-tasks: ${TASK_DIR} does not exist; no task files to check`);
  process.exit(0);
}
const names = readdirSync(TASK_DIR).filter((n) => n.endsWith(".md")).sort();
if (names.length === 0) {
  console.log(`check-tasks: ${TASK_DIR} is empty; no task files to check`);
  process.exit(0);
}

// The grandfather list. '#' lines and blank lines are comments; every other
// line names one task file to skip. A "frozen YYYY-MM-DD" comment carries the
// date the list was frozen, shown in the PASS line.
const GRANDFATHER_FILE = join(TASK_DIR, ".schema-grandfathered");
const grandfathered = new Set();
let frozenDate = null;
if (existsSync(GRANDFATHER_FILE)) {
  for (const raw of readFileSync(GRANDFATHER_FILE, "utf8").split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "") continue;
    if (line.startsWith("#")) {
      const m = line.match(/frozen\s+(\d{4}-\d{2}-\d{2})/i);
      if (m && !frozenDate) frozenDate = m[1];
      continue;
    }
    grandfathered.add(line);
  }
}

function tableValue(lines, field) {
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^\|\s*([^|]+?)\s*\|\s*([^|]*?)\s*\|/);
    if (m && m[1].toLowerCase() === field.toLowerCase()) {
      return { value: m[2], n: i + 1 };
    }
  }
  return null;
}

function hasHeading(lines, field) {
  const re = new RegExp(`^#{2,6}\\s+${field.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\s|$)`, "i");
  return lines.some((l) => re.test(l));
}

function checkFile(name) {
  const rel = `tasks/${name}`;
  const failures = [];
  const fail = (line, message) => failures.push(`${rel}:${line}: ${message}`);
  const checkEnum = (entry, field, value, allowed) => {
    if (value.includes("/")) {
      fail(entry.n, `${field} is an unfilled template placeholder: "${value}"`);
      return;
    }
    if (!allowed.some((a) => a.toLowerCase() === value.toLowerCase())) {
      fail(entry.n, `${field} has invalid value "${value}" (allowed: ${allowed.join(", ")})`);
    }
  };

  const lines = readFileSync(join(TASK_DIR, name), "utf8").split(/\r?\n/);

  const h1Index = lines.findIndex((l) => /^#\s+\S/.test(l));
  if (h1Index < 0) {
    fail(1, "no H1 found");
  } else if (!/^#\s+TASK-/.test(lines[h1Index])) {
    fail(h1Index + 1, `H1 must begin "# TASK-": ${lines[h1Index]}`);
  }

  for (const field of TABLE_FIELDS) {
    const entry = tableValue(lines, field);
    if (!entry) {
      fail(1, `missing field: ${field}`);
      continue;
    }
    if (entry.value === "") {
      fail(entry.n, `empty field: ${field} (write "none" if empty)`);
      continue;
    }
    if (field === "Class") checkEnum(entry, field, entry.value, CLASS_VALUES);
    if (field === "Priority") checkEnum(entry, field, entry.value, PRIORITY_VALUES);
    if (field === "Readiness") checkEnum(entry, field, entry.value, READINESS_VALUES);
  }

  for (const field of TEXT_FIELDS) {
    if (!tableValue(lines, field) && !hasHeading(lines, field)) {
      fail(1, `missing field: ${field}`);
    }
  }
  return failures;
}

const failures = [];
const conforming = [];
for (const name of names) {
  const fileFailures = checkFile(name);
  if (!STRICT && grandfathered.has(name)) {
    if (fileFailures.length === 0) conforming.push(name);
    continue;
  }
  failures.push(...fileFailures);
}
if (!STRICT) {
  for (const name of conforming) {
    console.log(`check-tasks: WARN tasks/${name}: listed but conforms; remove the entry`);
  }
}
for (const f of failures) console.log(f);
if (failures.length > 0) {
  console.log(`check-tasks: FAIL (${failures.length})`);
  process.exit(1);
}
if (!STRICT && grandfathered.size > 0) {
  console.log(`check-tasks: PASS (${grandfathered.size} files grandfathered` +
    (frozenDate ? `, frozen ${frozenDate}` : "") + ")");
} else {
  console.log(`check-tasks: PASS (${names.length} task files checked)`);
}
