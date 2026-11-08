// Resolves the handoff layer root (the PowerShell twin lives beside this file).
// Order: SIMPLEHARN_HANDOFF, the sibling ../SimpleHarn_handoff directory, then
// the legacy <repo>/handoff. Returns null when none exists.

import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

export function handoffRoot(repoRoot) {
  const fromEnv = process.env.SIMPLEHARN_HANDOFF;
  if (fromEnv && existsSync(fromEnv)) return resolve(fromEnv);
  const sibling = join(dirname(resolve(repoRoot)), "SimpleHarn_handoff");
  if (existsSync(sibling)) return sibling;
  const legacy = join(resolve(repoRoot), "handoff");
  if (existsSync(legacy)) return legacy;
  return null;
}
