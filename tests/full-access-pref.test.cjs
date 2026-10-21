const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  readFullAccess,
  writeFullAccess,
  readAllFullAccess,
} = require("../src/app/fullAccessPref.ts");

const fake = (initial = {}) => {
  const data = { ...initial };
  return {
    getItem: (key) => (key in data ? data[key] : null),
    setItem: (key, value) => {
      data[key] = String(value);
    },
    data,
  };
};

test("the choice is remembered per agent and starts off", () => {
  const storage = fake();
  assert.deepEqual(readAllFullAccess(storage), { claude: false, codex: false });
  writeFullAccess("claude", true, storage);
  assert.equal(readFullAccess("claude", storage), true);
  assert.equal(readFullAccess("codex", storage), false);
  writeFullAccess("claude", false, storage);
  assert.equal(readFullAccess("claude", storage), false);
});

test("only the string true reads as on", () => {
  for (const value of ["1", "yes", "", "TRUE", "null"])
    assert.equal(
      readFullAccess("codex", fake({ "sushiai.fullAccess.codex": value })),
      false,
      `${value} should read as off`,
    );
});

test("an agent with no full-access mode stores nothing and reads off", () => {
  const storage = fake({ "sushiai.fullAccess.gemini": "true" });
  writeFullAccess("gemini", true, storage);
  assert.equal(readFullAccess("gemini", storage), false);
  assert.deepEqual(Object.keys(storage.data), ["sushiai.fullAccess.gemini"]);
});

test("a storage that throws reads as off and does not break a click", () => {
  const hostile = {
    getItem() {
      throw new Error("denied");
    },
    setItem() {
      throw new Error("denied");
    },
  };
  assert.equal(readFullAccess("claude", hostile), false);
  assert.doesNotThrow(() => writeFullAccess("claude", true, hostile));
});
