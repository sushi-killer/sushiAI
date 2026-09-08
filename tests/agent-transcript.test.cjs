const { test } = require("node:test");
const assert = require("node:assert/strict");
const library = import("../src/agents/transcript.ts");

test("blank content is dropped and runs of actions collapse into one block", async () => {
  const { visible, groupItems } = await library;
  const items = [
    { id: "1", kind: "text", role: "user", text: "Rename the release" },
    { id: "2", kind: "activity", output: { text: "" } },
    { id: "3", kind: "text", text: "   " },
    { id: "4", kind: "tool", name: "memory", output: { saved: true } },
    { id: "5", kind: "tool", name: "skill_manage", status: "running" },
    { id: "6", kind: "activity", name: "thinking.delta" },
    { id: "7", kind: "text", text: "Renamed it." },
  ];
  const kept = items.filter(visible);
  assert.deepEqual(
    kept.map((i) => i.id),
    ["1", "4", "5", "7"],
  );
  const grouped = groupItems(kept);
  assert.equal(grouped.length, 3);
  assert.equal(grouped[0].id, "1");
  assert.deepEqual(
    grouped[1].map((i) => i.id),
    ["4", "5"],
  );
  assert.equal(grouped[2].id, "7");
});

test("a lone action stays a plain row and an empty transcript stays empty", async () => {
  const { visible, groupItems } = await library;
  const grouped = groupItems(
    [{ id: "1", kind: "tool", name: "memory" }].filter(visible),
  );
  assert.equal(Array.isArray(grouped[0]), false);
  assert.equal(grouped[0].id, "1");
  assert.deepEqual(groupItems([].filter(visible)), []);
});

test("an image with no text is kept; a transcript item with nothing readable is not", async () => {
  const { visible } = await library;
  assert.equal(visible({ id: "1", kind: "image", name: "shot.png" }), true);
  assert.equal(visible({ id: "2", kind: "image" }), true);
  assert.equal(
    visible({ id: "3", kind: "notice", text: "", output: {} }),
    false,
  );
  assert.equal(visible({ id: "4", kind: "tool", effect: "Read file" }), true);
});

test("tool and event names read as words", async () => {
  const { label } = await library;
  assert.equal(label("skill_manage"), "Skills");
  assert.equal(label("web_search"), "Web search");
  assert.equal(label("delegate_task"), "Subagent");
  assert.equal(label("image.attach_bytes"), "Image attach bytes");
  assert.equal(label(""), "");
  assert.equal(label(undefined), "");
});

test("a raw stream event with no words leaves no row, unlike a named tool", async () => {
  const { visible } = await library;
  assert.equal(
    visible({ id: "1", kind: "activity", name: "thinking.delta" }),
    false,
  );
  assert.equal(
    visible({ id: "2", kind: "reasoning", name: "thinking" }),
    false,
  );
  assert.equal(
    visible({ id: "3", kind: "reasoning", text: "Weighing two options" }),
    true,
  );
  assert.equal(visible({ id: "4", kind: "tool", name: "memory" }), true);
  // Still empty while it claims to be running: an empty shell is never useful.
  assert.equal(
    visible({
      id: "5",
      kind: "activity",
      name: "thinking.delta",
      status: "running",
    }),
    false,
  );
  assert.equal(
    visible({ id: "6", kind: "tool", name: "memory", status: "running" }),
    true,
  );
});
