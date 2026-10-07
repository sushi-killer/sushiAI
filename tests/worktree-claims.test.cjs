const test = require("node:test");
const assert = require("node:assert/strict");

const claim = (over) => ({
  host: "local",
  repo: "/w/app",
  label: "claim",
  active: false,
  ...over,
});

test("one matcher: same host, path or branch in the repo, active claim first", async () => {
  const { claimFor } = await import("../src/lib/worktreeClaims.ts");
  const old = claim({ label: "old", path: "/w/app-wt" });
  const live = claim({ label: "live", path: "/w/app-wt/", active: true });
  assert.equal(
    claimFor([old, live], "local", { path: "/w/app-wt" }).label,
    "live",
  );
  assert.equal(claimFor([old], "ssh:lab", { path: "/w/app-wt" }), undefined);
  const byBranch = claim({ label: "b", branch: "t/1" });
  // The repository is named by its root or by its git common dir.
  assert.equal(
    claimFor([byBranch], "local", {
      path: "/x",
      branch: "t/1",
      repo: "/w/app/",
    }).label,
    "b",
  );
  assert.equal(
    claimFor([byBranch], "local", {
      path: "/x",
      branch: "t/1",
      commonDir: "/w/app/.git",
    }).label,
    "b",
  );
  assert.equal(
    claimFor([byBranch], "local", {
      path: "/x",
      branch: "t/1",
      repo: "/other",
    }),
    undefined,
  );
});

test("the sidebar and the Worktrees table agree on which claim a checkout has", async () => {
  const { memberClaim } = await import("../src/app/workspaceMerge.ts");
  const { worktreeRows } = await import("../src/projectWorktrees.ts");
  const old = claim({ label: "old", path: "/w/app-wt" });
  const live = claim({ label: "live", path: "/w/app-wt", active: true });
  const member = {
    hostKey: "local",
    git: { checkout: "/w/app-wt", branch: "t/1", commonDir: "/w/app/.git" },
  };
  assert.equal(memberClaim(member, [old, live]).label, "live");
  const rows = worktreeRows(
    [
      {
        host: "local",
        cwd: "/w/app",
        root: "/w/app",
        base: "main",
        worktrees: [{ path: "/w/app-wt", branch: "t/1" }],
      },
    ],
    [],
    [old, live],
    [],
  );
  assert.equal(rows[0].claim.label, "live");
});
