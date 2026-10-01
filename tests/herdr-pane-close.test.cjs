const { test } = require("node:test");
const assert = require("node:assert/strict");
const { closeHerdrPane } = require("../electron/herdr-pane-close.cjs");

function groupError() {
  return Object.assign(
    new Error("closing this pane would close a worktree group"),
    {
      code: "confirmation_required",
    },
  );
}

function snapshot(workspaces = [], panes = []) {
  return { snapshot: { workspaces, panes } };
}

test("ordinary pane closure sends exactly one close request", async () => {
  const calls = [];
  const response = { type: "ok" };
  const result = await closeHerdrPane(
    "/tmp/test.sock",
    "w1:p1",
    async (...args) => {
      calls.push(args);
      return args[1] === "session.snapshot" ? snapshot() : response;
    },
  );
  assert.equal(result, response);
  assert.deepEqual(calls, [
    ["/tmp/test.sock", "session.snapshot"],
    ["/tmp/test.sock", "pane.close", { pane_id: "w1:p1" }],
  ]);
});

test("a protected last primary pane closes independently of linked worktrees", async () => {
  const calls = [];
  const live = new Set(["w1:p1", "w2:p1"]);
  await closeHerdrPane(
    "/tmp/test.sock",
    "w1:p1",
    async (socket, method, params) => {
      calls.push({ socket, method, params });
      if (method === "session.snapshot") return snapshot();
      if (method === "pane.close" && params.pane_id === "w1:p1")
        throw groupError();
      if (method === "pane.move") {
        live.delete("w1:p1");
        live.add("w3:p1");
        return { move_result: { changed: true, pane: { pane_id: "w3:p1" } } };
      }
      live.delete(params.pane_id);
      return { type: "ok" };
    },
  );
  assert.deepEqual([...live], ["w2:p1"]);
  assert.deepEqual(
    calls.map(({ method, params }) => ({ method, params })),
    [
      { method: "session.snapshot", params: undefined },
      { method: "pane.close", params: { pane_id: "w1:p1" } },
      {
        method: "pane.move",
        params: {
          pane_id: "w1:p1",
          destination: { type: "new_workspace" },
          focus: false,
        },
      },
      { method: "pane.close", params: { pane_id: "w3:p1" } },
    ],
  );
});

test("unrelated errors never trigger a move or a group close", async () => {
  const expected = Object.assign(new Error("pane not found"), {
    code: "pane_not_found",
  });
  let calls = 0;
  await assert.rejects(
    closeHerdrPane("/tmp/test.sock", "w1:p1", async (socket, method) => {
      calls++;
      if (method === "session.snapshot") return snapshot();
      throw expected;
    }),
    (error) => error === expected,
  );
  assert.equal(calls, 2);
});

test("a refused move never closes another pane", async () => {
  const calls = [];
  await assert.rejects(
    closeHerdrPane("/tmp/test.sock", "w1:p1", async (socket, method) => {
      calls.push(method);
      if (method === "session.snapshot") return snapshot();
      if (method === "pane.close") throw groupError();
      return { move_result: { changed: false, reason: "same_tab" } };
    }),
    /could not detach/,
  );
  assert.deepEqual(calls, ["session.snapshot", "pane.close", "pane.move"]);
});

test("a zoomed primary unzooms once before detaching and preserves sibling and linked sessions", async () => {
  const calls = [];
  const live = new Set(["w1:p1", "w1:p2", "w2:p1"]);
  let zoomed = true;
  await closeHerdrPane(
    "/tmp/test.sock",
    "w1:p1",
    async (socket, method, params) => {
      calls.push({ method, params });
      if (method === "session.snapshot")
        return snapshot(
          [{ workspace_id: "w1", worktree: { is_linked_worktree: false } }],
          [{ pane_id: "w1:p1", workspace_id: "w1" }],
        );
      if (method === "pane.move") {
        if (zoomed)
          return { move_result: { changed: false, reason: "zoomed_tab" } };
        live.delete("w1:p1");
        live.add("w3:p1");
        return { move_result: { changed: true, pane: { pane_id: "w3:p1" } } };
      }
      if (method === "pane.zoom") {
        assert.deepEqual(params, { pane_id: "w1:p1", mode: "off" });
        zoomed = false;
        return { zoom: { changed: true, zoomed: false } };
      }
      assert.equal(params.pane_id, "w3:p1");
      live.delete(params.pane_id);
      return { type: "ok" };
    },
  );
  assert.deepEqual([...live], ["w1:p2", "w2:p1"]);
  assert.deepEqual(
    calls.map(({ method }) => method),
    ["session.snapshot", "pane.move", "pane.zoom", "pane.move", "pane.close"],
  );
});

test("a repeated move refusal after unzoom never closes the original pane", async () => {
  const calls = [];
  await assert.rejects(
    closeHerdrPane("/tmp/test.sock", "w1:p1", async (socket, method) => {
      calls.push(method);
      if (method === "session.snapshot")
        return snapshot(
          [{ workspace_id: "w1", worktree: { is_linked_worktree: false } }],
          [{ pane_id: "w1:p1", workspace_id: "w1" }],
        );
      if (method === "pane.zoom") return { zoom: { zoomed: false } };
      return { move_result: { changed: false, reason: "zoomed_tab" } };
    }),
    /could not detach/,
  );
  assert.deepEqual(calls, [
    "session.snapshot",
    "pane.move",
    "pane.zoom",
    "pane.move",
  ]);
});

test("an unzoom refusal or error never retries detach or closes any pane", async () => {
  for (const fail of [false, true]) {
    const calls = [];
    await assert.rejects(
      closeHerdrPane("/tmp/test.sock", "w1:p1", async (socket, method) => {
        calls.push(method);
        if (method === "session.snapshot")
          return snapshot(
            [{ workspace_id: "w1", worktree: { is_linked_worktree: false } }],
            [{ pane_id: "w1:p1", workspace_id: "w1" }],
          );
        if (method === "pane.zoom") {
          if (fail) throw new Error("unzoom refused");
          return {
            zoom: { changed: false, zoomed: true, reason: "single_pane" },
          };
        }
        return { move_result: { changed: false, reason: "zoomed_tab" } };
      }),
      fail ? /unzoom refused/ : /could not detach/,
    );
    assert.deepEqual(calls, ["session.snapshot", "pane.move", "pane.zoom"]);
  }
});

test("a primary's last pane detaches before close even with confirmation disabled", async () => {
  const calls = [];
  const live = new Set(["w1:p1", "w2:p1"]);
  await closeHerdrPane(
    "/tmp/test.sock",
    "w1:p1",
    async (socket, method, params) => {
      calls.push(method);
      if (method === "session.snapshot")
        return snapshot(
          [
            {
              workspace_id: "w1",
              pane_count: 1,
              worktree: { repo_key: "repo", is_linked_worktree: false },
            },
            {
              workspace_id: "w2",
              pane_count: 1,
              worktree: { repo_key: "repo", is_linked_worktree: true },
            },
          ],
          [{ pane_id: "w1:p1", workspace_id: "w1" }],
        );
      if (method === "pane.move") {
        live.delete("w1:p1");
        live.add("w3:p1");
        return { move_result: { changed: true, pane: { pane_id: "w3:p1" } } };
      }
      if (params.pane_id === "w1:p1") live.clear();
      else live.delete(params.pane_id);
      return { type: "ok" };
    },
  );
  assert.deepEqual([...live], ["w2:p1"]);
  assert.deepEqual(calls, ["session.snapshot", "pane.move", "pane.close"]);
});

test("a sibling exiting after the snapshot cannot make a primary close cascade", async () => {
  const live = new Set(["w1:p1", "w1:p2", "w2:p1"]);
  await closeHerdrPane(
    "/tmp/test.sock",
    "w1:p1",
    async (socket, method, params) => {
      if (method === "session.snapshot") {
        live.delete("w1:p2");
        return snapshot(
          [
            {
              workspace_id: "w1",
              pane_count: 2,
              worktree: { repo_key: "repo", is_linked_worktree: false },
            },
            {
              workspace_id: "w2",
              pane_count: 1,
              worktree: { repo_key: "repo", is_linked_worktree: true },
            },
          ],
          [
            { pane_id: "w1:p1", workspace_id: "w1" },
            { pane_id: "w1:p2", workspace_id: "w1" },
          ],
        );
      }
      if (method === "pane.move") {
        live.delete("w1:p1");
        live.add("w3:p1");
        return { move_result: { changed: true, pane: { pane_id: "w3:p1" } } };
      }
      if (params.pane_id === "w1:p1") live.clear();
      else live.delete(params.pane_id);
      return { type: "ok" };
    },
  );
  assert.deepEqual([...live], ["w2:p1"]);
});
