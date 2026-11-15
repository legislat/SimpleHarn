// Cardless gate for the engine adapter knob manifests.
//
// The manifest lives in `docs/reference/ENGINE_KNOBS.md`; each engine has a
// `## <engine>` section with a table:
//   | Engine key/flag | Support | Row key or forced reason |
// Support is one of typed | passthrough | forced. A `missing` or unclassified
// row fails. The check cannot discover a new engine key by itself; the audit
// and review are the discovery side, and this gate prevents a known gap from
// being recorded and then forgotten.
//
// Usage:
//   node scripts/check-engine-knobs.mjs
//   node scripts/check-engine-knobs.mjs --selftest

import { readFileSync } from "node:fs";

const ENGINES = ["llama", "ninfer", "strata", "xolotl", "basalt", "maya"];
const SUPPORT = new Set(["typed", "passthrough", "forced"]);
const DOC = "docs/reference/ENGINE_KNOBS.md";

function evaluate(text, engines = ENGINES) {
  const errors = [];
  for (const engine of engines) {
    const heading = new RegExp(`^##\\s+${engine}\\s*$`, "m");
    const match = heading.exec(text);
    if (!match) {
      errors.push(`${engine}: no manifest section`);
      continue;
    }
    const start = match.index + match[0].length;
    const rest = text.slice(start);
    const next = /^##\s+/m.exec(rest);
    const section = next ? rest.slice(0, next.index) : rest;
    let rows = 0;
    for (const line of section.split(/\r?\n/)) {
      if (!line.startsWith("|")) continue;
      const cells = line.split("|").map((cell) => cell.trim());
      if (cells.length < 4) continue;
      const key = cells[1];
      const support = cells[2].toLowerCase();
      if (key === "" || key === "Key" || key === "Key/flag" || key === "Engine key/flag" || /^-+$/.test(key)) continue;
      rows++;
      if (support === "missing" || support === "unknown" || support === "") {
        errors.push(`${engine}: unclassified/missing knob: ${key}`);
        continue;
      }
      if (!SUPPORT.has(support)) {
        errors.push(`${engine}: bad support \`${support}\` for ${key}`);
      }
      if (cells[3] === "") {
        errors.push(`${engine}: ${key} has no row key or forced reason`);
      }
    }
    if (rows === 0) errors.push(`${engine}: manifest has no rows`);
  }
  return errors;
}

function selfTest() {
  const clean = `## llama\n| Key | Support | Row key or forced reason |\n|---|---|---|\n| --temp | typed | temp: |\n`;
  const dirty = `## llama\n| Key | Support | Row key or forced reason |\n|---|---|---|\n| --new-knob | missing | - |\n`;
  const cleanErrors = evaluate(clean, ["llama"]);
  const dirtyErrors = evaluate(dirty, ["llama"]);
  if (cleanErrors.length !== 0) {
    throw new Error(`selftest clean fixture: ${cleanErrors.join("; ")}`);
  }
  if (dirtyErrors.length === 0) {
    throw new Error("selftest dirty fixture was not refused");
  }
}

try {
  selfTest();
} catch (error) {
  console.log(`FAIL ${error.message}`);
  process.exit(1);
}

if (process.argv.includes("--selftest")) {
  console.log("PASS selftest: clean manifest passes, missing knob fails");
  process.exit(0);
}

const text = readFileSync(DOC, "utf8");
const errors = evaluate(text);
if (errors.length > 0) {
  for (const error of errors) console.log(`FAIL ${error}`);
  console.log("ENGINE KNOBS FAIL");
  process.exit(1);
}
console.log(`ENGINE KNOBS PASS (${ENGINES.length} manifests)`);
