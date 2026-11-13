// Verify a published release body has no known rendering defects. Reads the
// body on stdin; the release-facts job pipes `gh release view --jq .body` into it.
//
// A literal `$linux` placeholder, a backslash-escaped table cell, or a control
// character other than newline/tab means the body was pasted from a broken
// template, not rendered by scripts/render-release-notes.ps1.

let text = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => (text += chunk));
process.stdin.on("end", () => {
  const failures = [];
  if (text.includes("$linux")) failures.push("literal $linux placeholder");
  if (text.includes("\\|")) failures.push("backslash-escaped table cell");
  if (/[\x00-\x08\x0B\x0C\x0E-\x1F]/.test(text)) failures.push("control character");
  if (failures.length > 0) {
    for (const failure of failures) console.log(`FAIL release body: ${failure}`);
    process.exit(1);
  }
  console.log("PASS release body has no placeholder, escaped cell, or control character");
});
