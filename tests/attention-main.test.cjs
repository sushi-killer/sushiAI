const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  normalizePreferences,
  closeAction,
  trayTitle,
  trayState,
  validateNotice,
} = require("../electron/attention.cjs");

test("normalizePreferences keeps valid booleans and falls back to defaults otherwise", () => {
  assert.deepEqual(
    normalizePreferences({ runInMenuBar: false, notifications: false }),
    {
      runInMenuBar: false,
      notifications: false,
    },
  );
  assert.deepEqual(normalizePreferences({}), {
    runInMenuBar: true,
    notifications: true,
  });
  assert.deepEqual(normalizePreferences(null), {
    runInMenuBar: true,
    notifications: true,
  });
  assert.deepEqual(normalizePreferences(undefined), {
    runInMenuBar: true,
    notifications: true,
  });
  assert.deepEqual(normalizePreferences("not an object"), {
    runInMenuBar: true,
    notifications: true,
  });
  assert.deepEqual(normalizePreferences([true, false]), {
    runInMenuBar: true,
    notifications: true,
  });
  // Non-boolean values for a known key fall back to the default for that key.
  assert.deepEqual(
    normalizePreferences({ runInMenuBar: "yes", notifications: false }),
    { runInMenuBar: true, notifications: false },
  );
  // Unknown keys are ignored rather than adopted.
  assert.deepEqual(normalizePreferences({ somethingElse: true }), {
    runInMenuBar: true,
    notifications: true,
  });
});

test("closeAction hides only while running in the menu bar and not quitting", () => {
  assert.equal(closeAction({ quitting: false, runInMenuBar: true }), "hide");
  assert.equal(closeAction({ quitting: true, runInMenuBar: true }), "close");
  assert.equal(closeAction({ quitting: false, runInMenuBar: false }), "close");
  assert.equal(closeAction({ quitting: true, runInMenuBar: false }), "close");
});

test("trayTitle shows the count only when positive", () => {
  assert.equal(trayTitle(0), "");
  assert.equal(trayTitle(-1), "");
  assert.equal(trayTitle(3), "3");
  assert.equal(trayTitle(3.9), "3");
  assert.equal(trayTitle(NaN), "");
  assert.equal(trayTitle(Infinity), "");
  assert.equal(trayTitle(undefined), "");
});

test("validateNotice accepts a well-formed notice and returns its fields", () => {
  const notice = {
    workspaceId: "workspace-1",
    panelId: "panel-1",
    title: "Agent needs input",
    body: "Waiting on your reply in the terminal.",
  };
  assert.deepEqual(validateNotice(notice), notice);
});

test("validateNotice rejects a non-object notice", () => {
  assert.throws(() => validateNotice(null));
  assert.throws(() => validateNotice(undefined));
  assert.throws(() => validateNotice("notice"));
  assert.throws(() => validateNotice([]));
});

test("validateNotice rejects a non-string ID", () => {
  assert.throws(() =>
    validateNotice({
      workspaceId: 42,
      panelId: "panel-1",
      title: "Title",
      body: "Body",
    }),
  );
  assert.throws(() =>
    validateNotice({
      workspaceId: "workspace-1",
      panelId: { id: "panel-1" },
      title: "Title",
      body: "Body",
    }),
  );
});

test("validateNotice rejects an oversized title", () => {
  assert.throws(() =>
    validateNotice({
      workspaceId: "workspace-1",
      panelId: "panel-1",
      title: "x".repeat(121),
      body: "Body",
    }),
  );
  // At the boundary is still valid.
  assert.doesNotThrow(() =>
    validateNotice({
      workspaceId: "workspace-1",
      panelId: "panel-1",
      title: "x".repeat(120),
      body: "Body",
    }),
  );
});

test("validateNotice rejects an oversized body and empty strings", () => {
  assert.throws(() =>
    validateNotice({
      workspaceId: "workspace-1",
      panelId: "panel-1",
      title: "Title",
      body: "x".repeat(301),
    }),
  );
  assert.throws(() =>
    validateNotice({
      workspaceId: "",
      panelId: "panel-1",
      title: "Title",
      body: "Body",
    }),
  );
});

test("trayState prefers waiting over working and reads a bad count as quiet", () => {
  assert.equal(trayState(0, 0), "idle");
  assert.equal(trayState(0, 4), "working");
  assert.equal(trayState(2, 0), "attention");
  assert.equal(trayState(2, 4), "attention");
  assert.equal(trayState(NaN, NaN), "idle");
  assert.equal(trayState(undefined, undefined), "idle");
  assert.equal(trayState(-1, -1), "idle");
});
