const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  createOwnerPrompts,
} = require("../electron/extensions/companion-prompts.cjs");
const { registerExtensionIpc } = require("../electron/ipc/extensions.cjs");

function rig(options = {}) {
  let clock = 0;
  const shown = [];
  const withdrawn = [];
  const prompts = createOwnerPrompts({
    announce: (card) => shown.push(card),
    withdraw: (card) => withdrawn.push(card.id),
    now: () => clock,
    ...options,
  });
  return { prompts, shown, withdrawn, advance: (ms) => (clock += ms) };
}
const question = (extensionId, title = "t") => ({ extensionId, title });
const tick = () => new Promise((resolve) => setTimeout(resolve, 5));

test("one card at a time across all companions, in order; each answer shows the next", async () => {
  const r = rig({ minShownMs: 0 });
  const a = r.prompts.ask(question("ext.a"));
  const b = r.prompts.ask(question("ext.b"));
  const c = r.prompts.ask(question("ext.a"));
  assert.equal(r.shown.length, 1);
  r.prompts.answer(r.shown[0].id, true);
  assert.equal(await a, "allowed");
  assert.deepEqual(r.withdrawn, [r.shown[0].id]);
  assert.equal(r.shown.length, 2);
  assert.equal(r.shown[1].extensionId, "ext.b");
  r.prompts.answer(r.shown[1].id, false);
  assert.equal(await b, "denied");
  r.prompts.answer(r.shown[2].id, true);
  assert.equal(await c, "allowed");
});

test("only the id on screen is accepted, once; an unknown id changes nothing", async () => {
  const r = rig({ minShownMs: 0 });
  const result = r.prompts.ask(question("ext.a"));
  r.prompts.answer("not-an-id", true);
  r.prompts.answer(undefined, true);
  assert.equal(r.withdrawn.length, 0);
  const { id } = r.shown[0];
  r.prompts.answer(id, false);
  assert.equal(await result, "denied");
  // A second answer to the same id is a no-op, not an Allow.
  r.prompts.answer(id, true);
  assert.equal(r.withdrawn.length, 1);
});

test("a click earlier than 1 s after the card was shown does not allow", async () => {
  const r = rig();
  let decided = null;
  const result = r.prompts.ask(question("ext.a")).then((v) => (decided = v));
  const { id } = r.shown[0];
  r.advance(999);
  r.prompts.answer(id, true);
  await tick();
  assert.equal(decided, null, "the card is still waiting");
  r.advance(1);
  r.prompts.answer(id, true);
  await result;
  assert.equal(decided, "allowed");
  // Deny is never gated.
  const second = r.prompts.ask(question("ext.a"));
  r.prompts.answer(r.shown[1].id, false);
  assert.equal(await second, "denied");
});

test("2 minutes after the card is shown it times out, is withdrawn, and a late answer is a no-op", async () => {
  const r = rig({ timeoutMs: 40, minShownMs: 0 });
  const first = r.prompts.ask(question("ext.a"));
  const second = r.prompts.ask(question("ext.a"));
  await tick();
  assert.equal(r.shown.length, 1);
  assert.equal(await first, "timeout");
  assert.deepEqual(r.withdrawn, [r.shown[0].id]);
  r.prompts.answer(r.shown[0].id, true); // late
  // The queued card starts its own clock when it is shown.
  assert.equal(r.shown.length, 2);
  assert.equal(await second, "timeout");
});

test("cancel voids the shown and the queued cards of one companion only", async () => {
  const r = rig({ minShownMs: 0 });
  const a1 = r.prompts.ask(question("ext.a"));
  const b1 = r.prompts.ask(question("ext.b"));
  const a2 = r.prompts.ask(question("ext.a"));
  r.prompts.cancel("ext.a");
  assert.equal(await a1, "denied");
  assert.equal(await a2, "denied");
  assert.equal(r.shown.at(-1).extensionId, "ext.b");
  r.prompts.answer(r.shown.at(-1).id, true);
  assert.equal(await b1, "allowed");
});

test("the IPC answer handler passes only ids through to the queue", async () => {
  const handlers = new Map();
  const shown = [];
  const asks = [];
  const companions = {
    setAskOwner(ask) {
      asks.push(ask);
    },
  };
  registerExtensionIpc({
    handle: (channel, fn) => handlers.set(channel, fn),
    getExtensions: () => ({ companions, onCompanionChanged() {} }),
    getSurfaceState: () => ({}),
    announceExec: (card) => shown.push(card),
  });
  const answer = handlers.get("extensions-companion-exec-answer");
  const result = asks[0](question("ext.a"));
  assert.equal(shown.length, 1);
  await answer("unknown", true);
  await answer(shown[0].id, "yes"); // not strictly true: a Deny
  assert.equal(await result, "denied");
  assert.throws(() => answer(42, true), /Invalid question id/);
  await answer(shown[0].id, true); // already answered
});
