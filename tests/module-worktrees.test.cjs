const test = require("node:test");
const assert = require("node:assert/strict");

test("tasks become local claims, active first then newest, blank fields dropped", async () => {
  const { claimsOf } = await import("../src/orchestrator/moduleWorktrees.ts");
  const base = { repo: "/w/app", branch: "", worktree: "" };
  const claims = claimsOf([
    { ...base, title: "old", status: "done", updatedAt: 9, worktree: "/w/a" },
    { ...base, title: "mid", status: "running", updatedAt: 2, branch: "t/1" },
    { ...base, title: "new", status: "waiting", updatedAt: 5 },
    { ...base, title: "gone", status: "failed", updatedAt: 1 },
  ]);
  assert.deepEqual(
    claims.map((c) => [c.label, c.active]),
    [
      ["new", true],
      ["mid", true],
      ["old", false],
      ["gone", false],
    ],
  );
  assert.deepEqual(claims[1], {
    host: "local",
    repo: "/w/app",
    path: undefined,
    branch: "t/1",
    label: "mid",
    active: true,
  });
  assert.equal(claims[2].path, "/w/a");
});

test("a remote task claims its worktree on its own host", async () => {
  const { claimsOf } = await import("../src/orchestrator/moduleWorktrees.ts");
  const [claim] = claimsOf([
    {
      repo: "/srv/app",
      branch: "t/2",
      worktree: "/srv/app-wt",
      title: "remote",
      status: "running",
      updatedAt: 1,
      host: "ssh:lab",
    },
  ]);
  assert.equal(claim.host, "ssh:lab");
});
