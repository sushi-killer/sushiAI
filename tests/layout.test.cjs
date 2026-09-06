const { test } = require("node:test");
const assert = require("node:assert/strict");
const library = import("../src/layout.ts");
const ids = (node) =>
  !node
    ? []
    : node.type === "leaf"
      ? [node.id]
      : [...ids(node.a), ...ids(node.b)];
test("moving a pane to an edge preserves each pane exactly once", async () => {
  const { tidy, remove, insert } = await library;
  const original = tidy(["a", "b", "c", "d"]);
  const moved = insert(remove(original, "c"), "a", "c", "left");
  assert.deepEqual(ids(moved).sort(), ["a", "b", "c", "d"]);
  assert.deepEqual(ids(moved.a), ["c", "a"]);
  assert.equal(moved.a.axis, "row");
});
test("closing the last leaf collapses empty splits without phantom panels", async () => {
  const { tidy, remove } = await library;
  let layout = tidy(["a", "b", "c"]);
  layout = remove(layout, "b");
  layout = remove(layout, "a");
  assert.deepEqual(layout, { type: "leaf", id: "c" });
  assert.equal(remove(layout, "c"), null);
});
test("swap keeps split geometry and resizing changes only its target", async () => {
  const { tidy, swap, resize } = await library;
  const original = tidy(["a", "b", "c"]);
  const swapped = swap(original, "a", "c");
  assert.equal(swapped.ratio, original.ratio);
  assert.deepEqual(ids(swapped), ["c", "b", "a"]);
  const resized = resize(swapped, swapped.b.id, 0.7);
  assert.equal(resized.ratio, original.ratio);
  assert.equal(resized.b.ratio, 0.7);
});
