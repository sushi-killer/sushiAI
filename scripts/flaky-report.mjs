// Flake accounting on main: every Rust test that passed only on its retry
// (FLAKY in the JUnit report) gets one open issue labelled `flaky`, and each
// further flake adds a comment, so the comment count is the test's flake
// count. Runs after the Rust tests of a push to main; never fails the job.
// Usage: node scripts/flaky-report.mjs <junit.xml> <run url>
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { parse } from "./nextest-summary.mjs";

export const LABEL = "flaky";
export const titleOf = (id) => `Flaky test: ${id}`;

/** The gh calls for one report: `issues` are the open `flaky` issues. */
export function plan(cases, issues, runUrl) {
  const calls = [];
  for (const c of cases.filter((c) => c.status === "flaky")) {
    const open = issues.find((i) => i.title === titleOf(c.id));
    const line = `Passed only on its retry in ${runUrl}`;
    calls.push(
      open
        ? ["issue", "comment", String(open.number), "--body", line]
        : [
            "issue",
            "create",
            "--title",
            titleOf(c.id),
            "--label",
            LABEL,
            "--body",
            `${line}\n\nEach comment below is one more flake on main. Fix the wait in the test (wait on a signal, not a sleep); quarantine only with an owner and an expiry (.config/flaky-quarantine.json).`,
          ],
    );
  }
  return calls;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [file, runUrl = ""] = process.argv.slice(2);
  try {
    if (!file || !fs.existsSync(file)) process.exit(0);
    const cases = parse(fs.readFileSync(file, "utf8"));
    const flaky = cases.filter((c) => c.status === "flaky").length;
    console.log(
      `flaky-report: ${flaky} of ${cases.length} tests passed only on a retry (${((100 * flaky) / Math.max(cases.length, 1)).toFixed(2)}%)`,
    );
    if (!flaky) process.exit(0);
    const gh = (args) => execFileSync("gh", args, { encoding: "utf8" });
    gh(["label", "create", LABEL, "--force", "--color", "d93f0b"]);
    const issues = JSON.parse(
      gh([
        "issue",
        "list",
        "--label",
        LABEL,
        "--state",
        "open",
        "--limit",
        "500",
        "--json",
        "number,title",
      ]),
    );
    for (const call of plan(cases, issues, runUrl)) gh(call);
  } catch (error) {
    // Accounting must never turn main red; the warning shows it broke.
    console.log(`::warning title=flaky-report::${error.message}`);
  }
}
