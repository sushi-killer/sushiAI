const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  DEV_RESTART_EXIT_CODE,
  isCoreSource,
  exitAction,
} = require("../electron/dev-restart.cjs");

test("the restart exit code is 75", () => {
  assert.equal(DEV_RESTART_EXIT_CODE, 75);
});

test("isCoreSource accepts code files only", () => {
  for (const name of ["a.cjs", "b.mjs", "c.js", "x/y.py"])
    assert.equal(isCoreSource(name), true, name);
  for (const name of ["a.json", "b.md", "", null, undefined])
    assert.equal(isCoreSource(name), false, String(name));
});

test("exitAction respawns only on 75", () => {
  assert.equal(exitAction(75), "respawn");
  for (const code of [0, 1, null]) assert.equal(exitAction(code), "exit");
});
