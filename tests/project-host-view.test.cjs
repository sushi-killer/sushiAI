const test = require("node:test");
const assert = require("node:assert/strict");

test("host view helpers", async () => {
  const { hostDots, overridesToText, textToOverrides } =
    await import("../src/projectHostView.ts");
  const { tildePath } = await import("../src/projectPrepare.ts");
  const ready = {
    checkout: { ok: true, path: "/w", nonStandard: false },
    setup: { ok: true, configured: true },
    clis: {
      git: true,
      claude: { installed: true, loggedIn: true },
      codex: { installed: false, loggedIn: false },
      checkedAt: 0,
    },
    mcp: { ok: true, count: 2 },
    secrets: { ok: true, count: 3 },
    trusted: true,
  };
  assert.deepEqual(hostDots(ready, true).problems, []);
  const bare = {
    ...ready,
    checkout: { ok: false, path: "", nonStandard: false },
  };
  const dots = hostDots(bare, false);
  assert.equal(dots.checkout, "danger");
  assert.equal(dots.setup, null);
  assert.deepEqual(dots.problems, ["secrets"]);

  assert.equal(tildePath("/home/u/app", "/home/u"), "~/app");
  assert.equal(tildePath("/srv/app", "/home/u"), "/srv/app");

  const text = overridesToText({ A: "1", B: "x y" });
  assert.deepEqual(textToOverrides(text), { A: "1", B: "x y" });
  assert.throws(() => textToOverrides("nokey"), /KEY=value/);
  assert.throws(() => textToOverrides("1A=x"), /valid variable name/);
});
