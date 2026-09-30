const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/layout.ts");
const state = import("../src/workspaceState.ts");

test("isValidLayout accepts a well-formed leaf or split tree", async () => {
  const { isValidLayout } = await library;
  assert.equal(isValidLayout({ type: "leaf", id: "p-1" }), true);
  assert.equal(
    isValidLayout({
      type: "split",
      id: "s-1",
      axis: "row",
      ratio: 0.5,
      a: { type: "leaf", id: "p-1" },
      b: { type: "leaf", id: "p-2" },
    }),
    true,
  );
  // Distractor: a valid split whose branch is itself a valid split - depth
  // must not trip up the recursion.
  assert.equal(
    isValidLayout({
      type: "split",
      id: "s-2",
      axis: "column",
      ratio: 0.3,
      a: { type: "leaf", id: "p-1" },
      b: {
        type: "split",
        id: "s-3",
        axis: "row",
        ratio: 0.6,
        a: { type: "leaf", id: "p-2" },
        b: { type: "leaf", id: "p-3" },
      },
    }),
    true,
  );
});

test("isValidLayout rejects anything that isn't a real leaf/split tree", async () => {
  const { isValidLayout } = await library;
  assert.equal(isValidLayout(null), false);
  assert.equal(isValidLayout(undefined), false);
  assert.equal(isValidLayout("x"), false);
  assert.equal(
    isValidLayout({ g: "x" }),
    false,
    "a stray shape from a bad format",
  );
  assert.equal(
    isValidLayout({ type: "leaf" }),
    false,
    "a leaf needs a string id",
  );
  assert.equal(isValidLayout({ type: "leaf", id: 7 }), false);
  assert.equal(
    isValidLayout({
      type: "split",
      id: "s-1",
      axis: "diagonal",
      ratio: 0.5,
      a: { type: "leaf", id: "p-1" },
      b: { type: "leaf", id: "p-2" },
    }),
    false,
    "axis must be row or column",
  );
  assert.equal(
    isValidLayout({
      type: "split",
      id: "s-1",
      axis: "row",
      ratio: 0.5,
      a: { type: "leaf", id: "p-1" },
      b: { g: "x" },
    }),
    false,
    "a malformed branch anywhere in the tree fails the whole node",
  );
});

test("the snapshot keeps valid merged-group layouts and drops malformed ones on restore", async () => {
  const { restore } = await state;
  const good = {
    type: "split",
    id: "s-1",
    axis: "column",
    ratio: 0.3,
    a: { type: "leaf", id: "p-1" },
    b: { type: "leaf", id: "p-2" },
  };
  const workspace = {
    id: "w",
    name: "w",
    cwd: "/a",
    panels: [],
    layout: null,
  };
  const read = (mergedLayouts) => ({
    read: () =>
      JSON.stringify({ workspaces: [workspace], socket: "", mergedLayouts }),
  });
  assert.deepEqual(
    restore(
      read({
        "group-a": good,
        "group-b": { type: "split", id: "s", axis: "diagonal" },
        "group-c": "nope",
        "group-d": { type: "leaf" },
      }),
    ).mergedLayouts,
    { "group-a": good },
  );
  assert.deepEqual(restore(read(undefined)).mergedLayouts, {});
  assert.deepEqual(restore(read([good])).mergedLayouts, {});
  assert.deepEqual(restore(read("bad")).mergedLayouts, {});
});
