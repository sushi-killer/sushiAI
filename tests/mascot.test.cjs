const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  MASCOT_TIMED_MS,
  MASCOT_CONFIRMED_MS,
  MASCOT_MAX_NOTICES,
  MAX_REPLY_CHARS,
  noticeLifetimeMs,
  queueReducer,
  validateAction,
  mascotBounds,
  MAX_HEIGHT_RATIO,
} = require("../electron/mascot.cjs");

const notice = (kind, key, extra = {}) => ({
  source: "src",
  key,
  kind,
  title: "t",
  body: "b",
  ...extra,
});
const add = (state, n, now = 1000) =>
  queueReducer(state, { type: "add", notice: n, now });
const ids = (state) => state.map((item) => item.id);

test("the queue puts the newest notice first and caps its length", () => {
  let state = [];
  for (const key of ["a", "b", "c", "d", "e", "f", "g"])
    state = add(state, notice("done", key));
  assert.equal(MASCOT_MAX_NOTICES, 5);
  assert.deepEqual(ids(state), ["src:g", "src:f", "src:e", "src:d", "src:c"]);
});

test("a notice is one entry per source and key and moves to the front", () => {
  let state = [];
  for (const key of ["a", "b"]) state = add(state, notice("done", key));
  state = add(state, notice("done", "a"), 2000);
  assert.deepEqual(ids(state), ["src:a", "src:b"]);
  assert.equal(state[0].expiresAt, 2000 + MASCOT_TIMED_MS);
  state = add(state, notice("done", "a-other"));
  assert.equal(state.length, 3);
  // The same key from another source is another entry.
  state = add(state, notice("done", "a", { source: "other" }));
  assert.deepEqual(ids(state).slice(0, 2), ["other:a", "src:a-other"]);
  assert.equal(state.length, 4);
});

test("replacing a notice drops its confirmed state", () => {
  let state = add([], notice("input", "a", { body: "older" }), 1000);
  state = add(state, notice("input", "b"), 1100);
  state = queueReducer(state, {
    type: "confirm",
    id: "src:a",
    message: "Sent",
    now: 1400,
  });
  state = add(state, notice("input", "a", { body: "newer" }), 1500);
  const a = state.filter((item) => item.key === "a");
  assert.equal(a.length, 1);
  assert.equal(a[0].body, "newer");
  assert.equal(a[0].confirmed, undefined);
  assert.equal(a[0].expiresAt, null);
  assert.equal(state[0], a[0]);
  assert.ok(state.some((item) => item.key === "b"));
});

test("dismiss removes one notice by id; clear removes a source or all non-sticky", () => {
  let state = add(add([], notice("done", "a")), notice("failed", "b"));
  state = queueReducer(state, { type: "dismiss", id: state[0].id });
  assert.deepEqual(ids(state), ["src:a"]);
  assert.deepEqual(queueReducer(state, { type: "clear" }), []);
  const mixed = add(state, notice("info", "x", { source: "other" }));
  assert.deepEqual(ids(queueReducer(mixed, { type: "clear", source: "src" })), [
    "other:x",
  ]);
});

test("lifetimes: input and sticky wait, others last 10000 ms", () => {
  assert.equal(MASCOT_TIMED_MS, 10000);
  assert.equal(noticeLifetimeMs(notice("input", "a")), null);
  assert.equal(noticeLifetimeMs(notice("info", "a", { sticky: true })), null);
  for (const kind of ["done", "failed", "info"])
    assert.equal(noticeLifetimeMs(notice(kind, "a")), 10000);
  let state = add(
    add(add([], notice("input", "a")), notice("done", "b")),
    notice("failed", "c"),
  );
  assert.equal(state.find((i) => i.kind === "input").expiresAt, null);
  state = queueReducer(state, { type: "expire", now: 1000 + 9999 });
  assert.equal(state.length, 3);
  state = queueReducer(state, { type: "expire", now: 1000 + 10000 });
  assert.deepEqual(ids(state), ["src:a"]);
  assert.equal(queueReducer(state, { type: "expire", now: 1e15 }).length, 1);
});

test("retract removes a source's key but leaves a confirmed notice its short life", () => {
  let state = add(
    add(add([], notice("input", "a")), notice("input", "b")),
    notice("done", "a2"),
  );
  state = queueReducer(state, { type: "retract", source: "src", key: "a" });
  assert.deepEqual(ids(state), ["src:a2", "src:b"]);
  state = queueReducer(state, { type: "retract", source: "other", key: "b" });
  assert.equal(state.length, 2);
  state = queueReducer(state, {
    type: "confirm",
    id: "src:b",
    message: "Sent",
    now: 5000,
  });
  assert.equal(
    state.find((i) => i.key === "b").expiresAt,
    5000 + MASCOT_CONFIRMED_MS,
  );
  state = queueReducer(state, { type: "retract", source: "src", key: "b" });
  assert.equal(state.length, 2);
  state = queueReducer(state, {
    type: "expire",
    now: 5000 + MASCOT_CONFIRMED_MS,
  });
  assert.deepEqual(ids(state), ["src:a2"]);
});

const reply = notice("input", "a", {
  reply: true,
  actions: [{ id: "open", label: "Open" }],
});
const queue = add([], reply);

test("validateAction accepts a queued notice's action and a trimmed reply", () => {
  assert.deepEqual(
    validateAction("src:a", "open", undefined, queue).text,
    undefined,
  );
  assert.equal(validateAction("src:a", "reply", "  yes  ", queue).text, "yes");
  assert.equal(
    validateAction("src:a", "reply", "x".repeat(MAX_REPLY_CHARS), queue).text
      .length,
    MAX_REPLY_CHARS,
  );
  assert.equal(
    validateAction("src:a", "open", undefined, queue).entry.key,
    "a",
  );
});

test("validateAction rejects every bad case", () => {
  assert.equal(MAX_REPLY_CHARS, 2000);
  const bad = (re, ...args) => assert.throws(() => validateAction(...args), re);
  bad(/Invalid notice/, 7, "open", undefined, queue);
  bad(/Invalid notice/, "", "open", undefined, queue);
  bad(/gone/, "src:zzz", "open", undefined, queue);
  bad(/gone/, "src:a", "open", undefined, []);
  bad(/gone/, "src:a", "open");
  bad(/Invalid action/, "src:a", "Open", undefined, queue);
  bad(/Invalid action/, "src:a", "../x", undefined, queue);
  bad(/Invalid action/, "src:a", 5, undefined, queue);
  bad(/Unknown action/, "src:a", "other", undefined, queue);
  bad(/Invalid reply/, "src:a", "reply", 42, queue);
  bad(/Invalid reply/, "src:a", "open", 42, queue);
  bad(/Type a reply/, "src:a", "reply", "   \n ", queue);
  bad(/too long/, "src:a", "reply", "x".repeat(MAX_REPLY_CHARS + 1), queue);
  const plain = add(
    [],
    notice("done", "p", { actions: [{ id: "open", label: "Open" }] }),
  );
  bad(/takes no reply/, "src:p", "reply", "yes", plain);
  const confirmed = queueReducer(queue, {
    type: "confirm",
    id: "src:a",
    message: "Sent",
    now: 1,
  });
  bad(/gone/, "src:a", "reply", "yes", confirmed);
});

test("mascotBounds hugs the bottom-right of the work area and grows for input", () => {
  const area = { x: 0, y: 25, width: 1440, height: 875 };
  const timed = mascotBounds(area, add([], notice("done", "a")));
  const input = mascotBounds(area, add([], notice("input", "a")));
  for (const b of [timed, input]) {
    assert.equal(b.x + b.width, 1440 - 12);
    assert.equal(b.y + b.height, 900 - 12);
  }
  assert.ok(input.height > timed.height);
});

test("mascotBounds clamps a reported content height and stays bottom-right", () => {
  const area = { x: 0, y: 25, width: 1440, height: 875 };
  const cap = Math.floor(MAX_HEIGHT_RATIO * 875);
  const queue = add([], notice("input", "a"));
  const min = mascotBounds(area, queue).height;
  const cases = [
    [100, min],
    [500, 500],
    [5000, cap],
    [undefined, min],
    [NaN, min],
    [Infinity, min],
    [-20, min],
    [0, min],
    ["600", min],
  ];
  for (const [given, expected] of cases) {
    const b = mascotBounds(area, queue, given);
    assert.equal(b.height, expected, String(given));
    assert.equal(b.x + b.width, 1440 - 12);
    assert.equal(b.y + b.height, 900 - 12);
  }
  const tiny = { x: 0, y: 0, width: 800, height: 300 };
  assert.equal(mascotBounds(tiny, queue, 5000).height, min);
});

test("a sticky notice survives a global clear, expiry and a retract of other keys", () => {
  const sticky = notice("info", "keep", { sticky: true });
  let state = add([], notice("done", "a"));
  state = add(state, sticky);
  state = add(state, { ...sticky, body: "again" });
  assert.equal(state.filter((item) => item.sticky).length, 1);
  assert.equal(state[0].expiresAt, null);
  assert.equal(state[0].body, "again");
  for (const action of [
    { type: "retract", source: "src", key: "a" },
    { type: "expire", now: 1e15 },
    { type: "clear" },
  ])
    assert.ok(
      queueReducer(state, action).some((item) => item.id === "src:keep"),
      action.type,
    );
  assert.deepEqual(queueReducer(state, { type: "clear" }).length, 1);
  assert.ok(
    !queueReducer(state, { type: "dismiss", id: "src:keep" }).some(
      (item) => item.id === "src:keep",
    ),
  );
  assert.ok(
    !queueReducer(state, { type: "clear", source: "src" }).some(
      (item) => item.id === "src:keep",
    ),
  );
});
