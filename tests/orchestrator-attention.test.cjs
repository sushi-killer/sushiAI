const { test } = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/orchestrator/moduleAttention.ts");
const task = (over = {}) => ({
  id: "t1",
  title: "T",
  repo: "/w/app",
  status: "done",
  archived: false,
  decisions: [],
  attempts: [],
  updatedAt: 1000,
  baseRef: "main",
  ...over,
});

test("tasks become answer, decide and review items, newest first within a kind", async () => {
  const { attentionItems } = await load();
  const items = attentionItems(
    [
      task({ id: "q", status: "waiting", updatedAt: 5 }),
      task({ id: "f", status: "failed", updatedAt: 6 }),
      task({ id: "d", status: "done", updatedAt: 7 }),
      task({ id: "r", status: "running" }),
      task({ id: "a", status: "failed", archived: true }),
    ],
    true,
  );
  assert.deepEqual(
    items.map((item) => [item.kind, item.key]),
    [
      ["decide", "task:f"],
      ["answer", "task:q"],
      ["review", "land:d"],
    ],
  );
  assert.equal(items[0].project, "app");
  assert.equal(items[0].host, "local");
  assert.equal(items[0].at, 6);
});

test("a remote-host task is filed under its host, not Local", async () => {
  const { attentionItems } = await load();
  const [item] = attentionItems(
    [task({ status: "waiting", host: "ssh:lab" })],
    true,
  );
  assert.equal(item.host, "ssh:lab");
});

test("the search text covers the title and the repo", async () => {
  const { attentionItems } = await load();
  const [item] = attentionItems(
    [task({ title: "Ship It", repo: "/w/Alpha" })],
    true,
  );
  assert.ok(item.search.includes("ship it"));
  assert.ok(item.search.includes("alpha"));
});

test("a landed, archived or child task is not a review item", async () => {
  const { attentionItems } = await load();
  assert.deepEqual(
    attentionItems(
      [
        task({ id: "l", landedSha: "abc" }),
        task({ id: "a", archived: true }),
        task({ id: "c", parent: "p" }),
      ],
      true,
    ),
    [],
  );
});

test("with the module off there are no items", async () => {
  const { attentionItems } = await load();
  assert.deepEqual(attentionItems([task({ status: "waiting" })], false), []);
});

test("Enter after a digit pick sends it, even right after the selection moved; Enter alone never sends the preselection", async () => {
  const { inboxEnterAnswer } = await load();
  const picked = {
    pick: "Keep behind a flag",
    preselected: "Remove",
    note: "",
  };
  const none = { pick: undefined, preselected: "Remove", note: "" };
  assert.equal(
    inboxEnterAnswer(picked, 1000, 1150, 1350),
    "Keep behind a flag",
  );
  assert.equal(
    inboxEnterAnswer(picked, 1000, 1150, 1700),
    "Keep behind a flag",
  );
  assert.equal(inboxEnterAnswer(none, 1000, undefined, 1200), "");
  assert.equal(inboxEnterAnswer(none, 1000, undefined, 5000), "");
  assert.equal(inboxEnterAnswer(picked, 1000, 900, 1200), "");
  assert.equal(inboxEnterAnswer(picked, 1000, 900, 1600), "Keep behind a flag");
});

test("the branch box facts omit a zero cost and an unknown cap", async () => {
  const { diffFacts } = await load();
  assert.equal(diffFacts(3, 1, 4, 0.04), "3 files · attempt 1/4 · $0.04");
  assert.equal(diffFacts(1, 1, undefined, 0), "1 file · attempt 1");
  assert.equal(diffFacts(undefined, 0, 4, undefined), "");
});
