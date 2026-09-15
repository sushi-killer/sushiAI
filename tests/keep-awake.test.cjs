const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readKeepAwake, writeKeepAwake } = require("../src/app/useKeepAwake.ts");

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

test("the preference round-trips and defaults to off", () => {
  const storage = fake();
  assert.equal(readKeepAwake(storage), false);
  writeKeepAwake(true, storage);
  assert.equal(readKeepAwake(storage), true);
  writeKeepAwake(false, storage);
  assert.equal(readKeepAwake(storage), false);
});

test("anything that is not the string true reads as off", () => {
  // The value is written by us, but it is also a key anyone can set by hand
  // in devtools - a stray value must not be read as "hold the blocker".
  for (const value of ["1", "yes", "", "TRUE", "null"])
    assert.equal(
      readKeepAwake(fake({ "sushiai.keepAwake": value })),
      false,
      `${value} should read as off`,
    );
});

test("a storage that throws leaves the app usable", () => {
  // Private windows and cleared site data both throw on access. The setting
  // is a convenience; losing it must not take the app down with it.
  const hostile = {
    getItem() {
      throw new Error("denied");
    },
    setItem() {
      throw new Error("denied");
    },
  };
  assert.equal(readKeepAwake(hostile), false);
  assert.doesNotThrow(() => writeKeepAwake(true, hostile));
});
