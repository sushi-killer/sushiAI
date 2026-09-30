const { test } = require("node:test");
const assert = require("node:assert/strict");
const library = import("../src/terminal-links.ts");

const urls = async (lines) =>
  (await library).findTerminalLinks(lines).map((link) => link.url);

test("trailing punctuation is excluded", async () => {
  assert.deepEqual(await urls(["see https://example.com/a."]), [
    "https://example.com/a",
  ]);
  assert.deepEqual(await urls(['"http://example.com/x?y=1",']), [
    "http://example.com/x?y=1",
  ]);
  assert.deepEqual(await urls(["go to https://example.com/a!?;:"]), [
    "https://example.com/a",
  ]);
});

test("balanced parentheses stay, unbalanced trailing ones go", async () => {
  assert.deepEqual(await urls(["https://en.wikipedia.org/wiki/A_(b)"]), [
    "https://en.wikipedia.org/wiki/A_(b)",
  ]);
  assert.deepEqual(await urls(["(see https://example.com/a)"]), [
    "https://example.com/a",
  ]);
  assert.deepEqual(await urls(["[https://example.com/a]"]), [
    "https://example.com/a",
  ]);
  assert.deepEqual(await urls(["https://example.com/a[1]"]), [
    "https://example.com/a[1]",
  ]);
});

test("a URL wrapped across two and three rows is one link", async () => {
  const { findTerminalLinks } = await library;
  const two = findTerminalLinks(["say https://exam", "ple.com/path ok"]);
  assert.equal(two.length, 1);
  assert.equal(two[0].url, "https://example.com/path");
  assert.deepEqual(two[0].start, { row: 0, column: 4 });
  assert.deepEqual(two[0].end, { row: 1, column: 11 });
  const three = findTerminalLinks(["https://ab", "cdefghij", "klm"]);
  assert.equal(three.length, 1);
  assert.equal(three[0].url, "https://abcdefghijklm");
  assert.deepEqual(three[0].start, { row: 0, column: 0 });
  assert.deepEqual(three[0].end, { row: 2, column: 2 });
});

test("several URLs on one line are each found", async () => {
  const { findTerminalLinks } = await library;
  const found = findTerminalLinks(["a http://a.io, b https://b.io/x c"]);
  assert.deepEqual(
    found.map((link) => link.url),
    ["http://a.io", "https://b.io/x"],
  );
  assert.deepEqual(found[0].start, { row: 0, column: 2 });
  assert.deepEqual(found[0].end, { row: 0, column: 12 });
});

test("non-web schemes and credentials are rejected", async () => {
  for (const text of [
    "ftp://example.com/a",
    "file:///etc/passwd",
    "javascript:alert(1)",
    "http://user:pass@example.com/",
    "http://user@example.com/",
  ])
    assert.deepEqual(await urls([text]), [], text);
});

test("openTerminalLink needs the platform modifier and a web URL", async () => {
  const { openTerminalLink } = await library;
  const opened = [];
  global.window = {
    bridge: {
      agentOpenExternal: async (url) => {
        opened.push(url);
      },
    },
  };
  const mac = /Mac|iPhone|iPad/i.test(
    navigator.platform || navigator.userAgent,
  );
  const modifier = mac ? "metaKey" : "ctrlKey";
  openTerminalLink({}, "https://example.com/");
  openTerminalLink({ [modifier]: true }, "file:///etc/passwd");
  openTerminalLink({ [modifier]: true }, "https://example.com/ok");
  delete global.window;
  assert.deepEqual(opened, ["https://example.com/ok"]);
});
