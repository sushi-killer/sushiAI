const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

// orchd's prompt and brief texts live in crates/sushiai-orch/prompts/*.yaml,
// compiled in with include_str!. The Rust code keeps only the assembly: which
// blocks go in, in what order, and the values filled in.
const ROOT = path.join(__dirname, "..");
const CRATE = path.join(ROOT, "crates", "sushiai-orch");
const SRC = path.join(CRATE, "src");
// A string constant this long is prompt text; so is a multi-line one this long.
const LONG_CONST = 100;
const LONG_MULTILINE_CONST = 40;
// Brief assemblers keep instruction text in the compiled YAML blocks.
const LONG_BRIEF_LITERAL = 100;
const BRIEF_SOURCES = [
  "engine/attempts/advisor.rs",
  "engine/attempts/brief.rs",
  "engine/attempts/context.rs",
  "engine/attempts/dependencies.rs",
  "engine/attempts/pick.rs",
  "engine/attempts/report.rs",
  "engine/attempts/resume.rs",
  "engine/planning/draft.rs",
  "engine/planning/prompt.rs",
  "engine/planning/serde.rs",
  "verify/review/brief.rs",
  "verify/review/reply.rs",
  "verify/review/routing.rs",
  "verify/evidence.rs",
  "delivery/brief.rs",
  "engine/audit/brief.rs",
  "prompts/evolution/proposal.rs",
  "prompts/reply.rs",
  "prompts/text.rs",
];
const CONST_DECLARATION =
  /(?:const|static)\s+[A-Z][A-Z0-9_]*\s*:\s*&\s*(?:'static\s+)?str\s*=\s*$/;
const RAW_STRING = /^b?r(#*)"/;
const CHAR_LITERAL =
  /^'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'/u;
const IDENTIFIER_CHAR = /[A-Za-z0-9_]/;
const INCLUDE_STR = /include_str!\(\s*"([^"]*)"\s*\)/g;

const count = (text, char) => text.split(char).length - 1;

function rustFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) return rustFiles(full);
    return entry.name.endsWith(".rs") ? [full] : [];
  });
}

// The crate's shipped sources: not the engine's test files.
function shippedFiles() {
  return rustFiles(SRC).filter((file) => {
    const rel = path.relative(SRC, file).split(path.sep).join("/");
    return (
      !rel.split("/").includes("tests") &&
      !rel.endsWith("/tests.rs") &&
      rel !== "engine/test_support.rs" &&
      !rel.endsWith("/test_fixtures.rs")
    );
  });
}

// The source with every #[cfg(test)] item (a test module, a test-only fn,
// use or const) blanked out; line numbers stay as they were.
function withoutTestItems(text) {
  const lines = text.split("\n");
  for (let start = 0; start < lines.length; start++) {
    if (lines[start].trim() !== "#[cfg(test)]") continue;
    const indent = lines[start].slice(0, lines[start].indexOf("#"));
    let end = start + 1;
    while (end < lines.length && lines[end].trim().startsWith("#[")) end++;
    for (; end < lines.length; end++) {
      const line = lines[end].trim();
      if (line.endsWith(";")) break;
      if (!line.includes("{")) continue;
      if (line.endsWith("}") && count(line, "{") === count(line, "}")) break;
      end++;
      while (
        end < lines.length &&
        lines[end] !== `${indent}}` &&
        lines[end] !== `${indent}};`
      )
        end++;
      break;
    }
    for (let i = start; i <= end && i < lines.length; i++) lines[i] = "";
    start = end;
  }
  return lines.join("\n");
}

// Every string literal outside comments: where it starts and its body as
// written in the source (escapes not decoded).
function stringLiterals(source) {
  const found = [];
  let i = 0;
  while (i < source.length) {
    const rest = source.slice(i, i + 64);
    if (rest.startsWith("//")) {
      const end = source.indexOf("\n", i);
      i = end === -1 ? source.length : end;
      continue;
    }
    if (rest.startsWith("/*")) {
      const end = source.indexOf("*/", i + 2);
      i = end === -1 ? source.length : end + 2;
      continue;
    }
    const raw = RAW_STRING.exec(rest);
    if (raw && !IDENTIFIER_CHAR.test(source[i - 1] || "")) {
      const open = i + raw[0].length;
      const close = source.indexOf(`"${raw[1]}`, open);
      const end = close === -1 ? source.length : close;
      found.push({ at: i, body: source.slice(open, end) });
      i = end + 1 + raw[1].length;
      continue;
    }
    if (source[i] === '"') {
      let end = i + 1;
      while (end < source.length && source[end] !== '"')
        end += source[end] === "\\" ? 2 : 1;
      found.push({ at: i, body: source.slice(i + 1, end) });
      i = end + 1;
      continue;
    }
    if (source[i] === "'") {
      const literal = CHAR_LITERAL.exec(rest);
      i += literal ? literal[0].length : 1;
      continue;
    }
    i++;
  }
  return found;
}

// The prompt texts left in a file's shipped code, as `path:line: start`.
function promptTexts(file, everyLiteral) {
  const source = withoutTestItems(fs.readFileSync(file, "utf8"));
  const rel = path.relative(ROOT, file).split(path.sep).join("/");
  return stringLiterals(source).flatMap(({ at, body }) => {
    const before = source.slice(Math.max(0, at - 200), at);
    const multiline = body.includes("\\n") || body.includes("\n");
    const isPrompt = CONST_DECLARATION.test(before)
      ? body.length >= LONG_CONST ||
        (multiline && body.length >= LONG_MULTILINE_CONST)
      : everyLiteral && body.length >= LONG_BRIEF_LITERAL;
    if (!isPrompt) return [];
    const line = count(source.slice(0, at), "\n") + 1;
    return [`${rel}:${line}: ${body.slice(0, 60)}`];
  });
}

test("brief assemblers keep no prompt text in their string literals", () => {
  assert.deepEqual(
    BRIEF_SOURCES.flatMap((file) => promptTexts(path.join(SRC, file), true)),
    [],
  );
});

test("the orchestrator crate keeps no prompt text in a string constant", () => {
  assert.deepEqual(
    shippedFiles().flatMap((file) => promptTexts(file, false)),
    [],
  );
});

test("every prompt YAML file of the orchestrator crate is compiled in", () => {
  const sources = shippedFiles()
    .map((file) => fs.readFileSync(file, "utf8"))
    .join("\n");
  const included = [...sources.matchAll(INCLUDE_STR)].map((match) => match[1]);
  const missing = fs
    .readdirSync(path.join(CRATE, "prompts"))
    .filter((name) => name.endsWith(".yaml"))
    .filter(
      (name) => !included.some((file) => file.endsWith(`prompts/${name}`)),
    );
  assert.deepEqual(missing, []);
});
