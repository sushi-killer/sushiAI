const { test } = require("node:test");
const assert = require("node:assert/strict");
const { placePrimaryNav } = require("../src/app/sidebarOrder.ts");

const names = (items) => items.map((item) => item.label);
const builtin = ["Inbox", "Dashboard", "Extensions", "Skills"].map((label) => ({
  label,
  order: 0,
}));

test("a negative order sits right under Inbox, above Dashboard", () => {
  const result = placePrimaryNav(builtin, [
    { label: "Notes", order: 0 },
    { label: "Routines", order: -10, builtin: true },
  ]);
  assert.deepEqual(names(result), [
    "Inbox",
    "Routines",
    "Dashboard",
    "Extensions",
    "Skills",
    "Notes",
  ]);
});

test("entries without a negative order stay after the app's sections", () => {
  const result = placePrimaryNav(builtin, [
    { label: "A", order: 0 },
    { label: "B", order: 5 },
  ]);
  assert.deepEqual(names(result).slice(-2), ["A", "B"]);
  assert.equal(names(result)[1], "Dashboard");
});

test("a third-party negative order does not jump above Dashboard", () => {
  const result = placePrimaryNav(builtin, [
    { label: "Notes", order: 0, builtin: true },
    { label: "Sneaky", order: -10, builtin: false },
  ]);
  assert.deepEqual(names(result), [
    "Inbox",
    "Dashboard",
    "Extensions",
    "Skills",
    "Notes",
    "Sneaky",
  ]);
});
