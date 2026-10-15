const test = require("node:test");
const assert = require("node:assert/strict");
const { canLaunchWorktree } = require("../src/workspace/worktree.ts");

const ready = { checkout: { ok: true } };

test("a local target always offers a worktree", () => {
  assert.equal(
    canLaunchWorktree({ setup: false, ssh: false, check: undefined }),
    true,
  );
});

test("a ready ssh host offers a worktree", () => {
  assert.equal(
    canLaunchWorktree({ setup: false, ssh: true, check: ready }),
    true,
  );
});

test("an ssh host that is not ready offers none", () => {
  for (const check of [
    undefined,
    "checking",
    "unreachable",
    { checkout: { ok: false } },
  ])
    assert.equal(
      canLaunchWorktree({ setup: false, ssh: true, check }),
      false,
      JSON.stringify(check),
    );
});

test("a host that is not set up offers none, local or ssh", () => {
  assert.equal(
    canLaunchWorktree({ setup: true, ssh: true, check: ready }),
    false,
  );
  assert.equal(
    canLaunchWorktree({ setup: true, ssh: false, check: undefined }),
    false,
  );
});
