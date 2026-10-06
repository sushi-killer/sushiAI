const test = require("node:test");
const assert = require("node:assert/strict");
const { serializePerHost } = require("../electron/host-setup.cjs");

test("setup for one host never overlaps, other hosts run in parallel", async () => {
  let active = 0;
  let peak = 0;
  const releases = [];
  const setup = serializePerHost(
    (endpoint) =>
      new Promise((resolve) => {
        active += 1;
        peak = Math.max(peak, active);
        releases.push(() => {
          active -= 1;
          resolve(endpoint);
        });
      }),
  );
  const a = setup("ssh:a", {});
  const b = setup("ssh:a", {});
  const other = setup("ssh:b", {});
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(releases.length, 2, "one for a, one for b");
  releases.shift()();
  await a;
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(releases.length, 2, "b's second call starts after the first");
  releases.splice(0).forEach((release) => release());
  await Promise.all([b, other]);
  assert.equal(peak, 2);
});
