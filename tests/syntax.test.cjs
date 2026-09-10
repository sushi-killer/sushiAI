const { test } = require("node:test");
const assert = require("node:assert/strict");

test("syntax highlighting keeps source text and colors common code tokens", async () => {
  const { highlightCode, languageFromPath } = await import("../src/syntax.ts");
  assert.equal(languageFromPath("src/App.tsx"), "typescript");
  assert.equal(languageFromPath("README.md"), "markdown");
  assert.equal(languageFromPath("notes.txt"), "plain");

  const source = 'const answer = fetch("/api"); // ready';
  const tokens = highlightCode(source, "src/App.tsx").flatMap(
    (line) => line.tokens,
  );
  assert.equal(tokens.map((token) => token.text).join(""), source);
  assert.equal(tokens.find((token) => token.text === "const").kind, "keyword");
  assert.equal(tokens.find((token) => token.text === '"/api"').kind, "string");
  assert.equal(tokens.find((token) => token.text === "fetch").kind, "function");
  assert.equal(
    tokens.find((token) => token.text === "// ready").kind,
    "comment",
  );
});

test("Git highlighting preserves diff markers and highlights only source payload", async () => {
  const { highlightDiff } = await import("../src/syntax.ts");
  const lines = highlightDiff(
    "@@ -1 +1 @@\n-const old = false;\n+const next = true;",
    "src/App.ts",
  );

  assert.deepEqual(
    lines.map((line) => line.diffKind),
    ["hunk", "deletion", "addition"],
  );
  assert.equal(lines[1].prefix, "-");
  assert.equal(lines[2].prefix, "+");
  assert.equal(
    lines[2].tokens.find((token) => token.text === "const").kind,
    "keyword",
  );
  assert.equal(
    lines[2].tokens.find((token) => token.text === "true").kind,
    "constant",
  );
});

test("SKILL.md frontmatter highlights YAML keys and scalar values", async () => {
  const { highlightCode } = await import("../src/syntax.ts");
  const source =
    "---\nname: skill-creator\ndescription: Create new skills for agents.\n---\n\n# Skill Creator";
  const lines = highlightCode(source, "SKILL.md");

  assert.equal(lines[0].tokens[0].kind, "punctuation");
  assert.equal(
    lines[1].tokens.find((token) => token.text === "name").kind,
    "property",
  );
  assert.equal(
    lines[1].tokens.find((token) => token.text === ":").kind,
    "punctuation",
  );
  assert.equal(
    lines[1].tokens.find((token) => token.text === "skill-creator").kind,
    "string",
  );
  assert.equal(
    lines[2].tokens.find((token) => token.text === "description").kind,
    "property",
  );
  assert.equal(
    lines[2].tokens.find(
      (token) => token.text === "Create new skills for agents.",
    ).kind,
    "string",
  );
  assert.equal(lines[3].tokens[0].kind, "punctuation");
  assert.equal(
    lines[5].tokens.find((token) => token.text === "# Skill Creator").kind,
    "heading",
  );
  assert.equal(
    lines
      .map((line) => line.tokens.map((token) => token.text).join(""))
      .join("\n"),
    source,
  );
});
