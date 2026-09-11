const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { Connections, run } = require("../electron/connections.cjs");

async function commit(root, name, content) {
  await fs.writeFile(path.join(root, name), content);
  await run("/usr/bin/git", ["-C", root, "add", name]);
  await run("/usr/bin/git", [
    "-C",
    root,
    "-c",
    "user.name=Tester",
    "-c",
    "user.email=tester@example.invalid",
    "commit",
    "-m",
    `add ${name}`,
  ]);
}

test("git log/commit/diff operations expose branch topology, refs and merge changes", async () => {
  const root = await fs.mkdtemp("/tmp/sushiai-git-history-test-");
  const c = new Connections(root);
  await c.init();
  const inspect = (operation, extra = {}) =>
    c.inspect(null, { operation, root, ...extra });
  try {
    await run("/usr/bin/git", ["init", "-q", "-b", "main", root]);
    await commit(root, "a.txt", "one\n");
    await run("/usr/bin/git", ["-C", root, "checkout", "-qb", "feature"]);
    await commit(root, "b.txt", "two\n");
    await run("/usr/bin/git", ["-C", root, "checkout", "-q", "main"]);
    await commit(root, "c.txt", "three\n");
    await run("/usr/bin/git", [
      "-C",
      root,
      "-c",
      "user.name=Tester",
      "-c",
      "user.email=tester@example.invalid",
      "merge",
      "-q",
      "--no-ff",
      "-m",
      "merge feature",
      "feature",
    ]);
    await run("/usr/bin/git", ["-C", root, "tag", "v1.0"]);
    // An unmerged branch: reachable only when browsing "all" refs, not "current".
    await run("/usr/bin/git", ["-C", root, "checkout", "-qb", "wip"]);
    await commit(root, "wip.txt", "unmerged\n");
    await run("/usr/bin/git", ["-C", root, "checkout", "-q", "main"]);

    const all = await inspect("log", { refs: "all" });
    assert.equal(all.head, "main");
    assert.equal(all.truncated, false);
    const subjects = all.commits.map((entry) => entry.subject);
    assert.deepEqual(
      new Set(subjects),
      new Set([
        "merge feature",
        "add wip.txt",
        "add b.txt",
        "add c.txt",
        "add a.txt",
      ]),
    );
    const merge = all.commits.find((entry) => entry.subject === "merge feature");
    assert.equal(merge.parents.length, 2);
    assert.ok(merge.refs.some((r) => r.includes("main")));
    assert.ok(merge.refs.some((r) => r.startsWith("tag: v1.0")));
    const rootCommit = all.commits.find((entry) => entry.subject === "add a.txt");
    assert.deepEqual(rootCommit.parents, []);

    const current = await inspect("log", { refs: "current" });
    const currentSubjects = current.commits.map((entry) => entry.subject);
    assert.ok(!currentSubjects.includes("add wip.txt"));
    assert.ok(currentSubjects.includes("merge feature"));
    assert.ok(currentSubjects.includes("add b.txt")); // merged-in ancestor, still reachable from HEAD

    // Browsing another branch must be read-only: it must not move HEAD.
    const featureHistory = await inspect("log", { branch: "feature" });
    assert.equal(featureHistory.head, "main");
    assert.ok(featureHistory.commits.some((entry) => entry.subject === "add b.txt"));
    assert.ok(!featureHistory.commits.some((entry) => entry.subject === "add c.txt"));
    assert.equal((await inspect("branches")).branches.find((entry) => entry.current).name, "main");

    const mainHistory = await inspect("log", { branch: "main" });
    assert.ok(mainHistory.commits.length > 0);

    const detail = await inspect("commit", { commit: merge.hash });
    assert.equal(detail.subject, "merge feature");
    assert.equal(detail.parents.length, 2);
    assert.deepEqual(detail.files, [{ status: "A", path: "b.txt" }]);

    const rootDetail = await inspect("commit", { commit: rootCommit.hash });
    assert.deepEqual(rootDetail.files, [{ status: "A", path: "a.txt" }]);

    const mergeDiff = await inspect("diff", {
      commit: merge.hash,
      path: "b.txt",
    });
    assert.match(mergeDiff.text, /\+two/);

    const rootDiff = await inspect("diff", {
      commit: rootCommit.hash,
      path: "a.txt",
    });
    assert.match(rootDiff.text, /\+one/);

    await assert.rejects(inspect("commit", { commit: "not-a-hash" }));
    await assert.rejects(inspect("diff", { commit: "'; rm -rf /", path: "a.txt" }));
  } finally {
    await c.close();
    await fs.rm(root, { recursive: true, force: true });
  }
});

test("branches lists local branches and checkout refuses to drop uncommitted work", async () => {
  const root = await fs.mkdtemp("/tmp/sushiai-git-branches-test-");
  const c = new Connections(root);
  await c.init();
  const inspect = (operation, extra = {}) =>
    c.inspect(null, { operation, root, ...extra });
  try {
    await run("/usr/bin/git", ["init", "-q", "-b", "main", root]);
    await commit(root, "a.txt", "one\n");
    await run("/usr/bin/git", ["-C", root, "checkout", "-qb", "feature"]);
    await commit(root, "b.txt", "two\n");
    await run("/usr/bin/git", ["-C", root, "checkout", "-q", "main"]);
    const featureHash = (
      await run("/usr/bin/git", ["-C", root, "rev-parse", "feature"])
    ).trim();
    await run("/usr/bin/git", [
      "-C",
      root,
      "update-ref",
      "refs/remotes/origin/feature",
      featureHash,
    ]);
    await run("/usr/bin/git", [
      "-C",
      root,
      "update-ref",
      "refs/remotes/origin/release",
      featureHash,
    ]);

    const before = await inspect("branches");
    assert.deepEqual(
      before.branches
        .filter((branch) => branch.local)
        .map((branch) => [branch.name, branch.current])
        .sort(),
      [
        ["feature", false],
        ["main", true],
      ],
    );
    assert.ok(before.branches.every((branch) => typeof branch.track === "string"));
    assert.equal(
      before.branches.find((branch) => branch.name === "feature").origin,
      "origin/feature",
    );
    const originOnly = before.branches.find((branch) => branch.name === "release");
    assert.equal(originOnly.ref, "origin/release");
    assert.equal(originOnly.local, false);
    assert.equal(originOnly.remoteOnly, true);
    assert.equal(originOnly.subject, "add b.txt");
    assert.ok(originOnly.date > 0);

    // A branch name that would be read as a git option, and one that is not a ref.
    await assert.rejects(inspect("checkout", { branch: "--upload-pack=x" }));
    await assert.rejects(inspect("checkout", { branch: "nope" }));
    await assert.rejects(inspect("checkout", { branch: "" }));

    await inspect("checkout", { branch: "feature" });
    const after = await inspect("branches");
    assert.equal(after.branches.find((branch) => branch.current).name, "feature");
    assert.equal((await inspect("log", { refs: "current" })).head, "feature");

    // Switching with uncommitted tracked changes must be refused, not carried over.
    await fs.writeFile(path.join(root, "b.txt"), "dirty\n");
    await assert.rejects(
      inspect("checkout", { branch: "main" }),
      /uncommitted changes/,
    );
    assert.equal((await inspect("branches")).branches.find((b) => b.current).name, "feature");
  } finally {
    await c.close();
    await fs.rm(root, { recursive: true, force: true });
  }
});
