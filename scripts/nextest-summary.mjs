// Reads the JUnit report of `cargo nextest run --profile ci` and makes the
// retried tests visible: a test that failed and then passed on its retry is
// FLAKY. Each one becomes a warning annotation, and the job summary
// ($GITHUB_STEP_SUMMARY) lists the flaky, failed and slowest tests.
// Usage: node scripts/nextest-summary.mjs <junit.xml> [label]
import fs from "node:fs";
import { fileURLToPath } from "node:url";

const attr = (tag, name) =>
  new RegExp(`\\b${name}="([^"]*)"`).exec(tag)?.[1] ?? "";
const unescape = (s) =>
  s
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");

/** Every test case: its id, seconds, and whether it failed, was flaky or passed. */
export function parse(xml) {
  const cases = [];
  const re = /<testcase\b([^>]*?)(\/>|>([\s\S]*?)<\/testcase>)/g;
  for (const [, head, , body = ""] of xml.matchAll(re)) {
    const id = unescape(`${attr(head, "classname")} ${attr(head, "name")}`);
    const status = /<(failure|error)\b/.test(body)
      ? "failed"
      : /<flaky(Failure|Error)\b/.test(body)
        ? "flaky"
        : /<skipped\b/.test(body)
          ? "skipped"
          : "passed";
    cases.push({ id, seconds: Number(attr(head, "time")) || 0, status });
  }
  return cases;
}

export function summary(cases, label = "") {
  const of = (status) => cases.filter((c) => c.status === status);
  const flaky = of("flaky");
  const failed = of("failed");
  const slow = [...cases].sort((a, b) => b.seconds - a.seconds).slice(0, 10);
  const lines = [
    `### Rust tests${label ? ` (${label})` : ""}`,
    "",
    `${cases.length} tests: ${failed.length} failed, ${flaky.length} FLAKY (passed only on the retry).`,
  ];
  const list = (title, items) => {
    if (!items.length) return;
    lines.push("", `**${title}**`, "");
    for (const c of items)
      lines.push(`- \`${c.id}\` ${c.seconds.toFixed(1)} s`);
  };
  list("FLAKY: fix these, a retry only hid them", flaky);
  list("Failed", failed);
  list("Slowest", slow);
  return { text: lines.join("\n") + "\n", flaky, failed };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [file, label = ""] = process.argv.slice(2);
  if (!file || !fs.existsSync(file)) {
    console.log(`nextest-summary: no JUnit report at ${file}`);
    process.exit(0);
  }
  const { text, flaky } = summary(parse(fs.readFileSync(file, "utf8")), label);
  for (const c of flaky)
    console.log(
      `::warning title=FLAKY Rust test::${c.id} passed only on its retry`,
    );
  if (process.env.GITHUB_STEP_SUMMARY)
    fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, text);
  console.log(text);
}
