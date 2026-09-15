const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/projects.ts");

const REMOTE = "example.test/team/app";

const profile = (id, name, extra = {}) => ({
  id,
  name,
  host: "192.0.2.10",
  socket: `ssh:${id}`,
  connected: true,
  ...extra,
});
const workspace = (id, connection, cwd, name = id) => ({
  id,
  name,
  cwd,
  connection,
  panels: [],
  layout: null,
});
const git = (remote, checkout, branch = "main") => ({
  remote,
  commonDir: `${checkout}/.git`,
  checkout,
  subdir: "",
  branch,
});
const closedProject = (id, name, cwd, endpoint, herdrFlag, gitId) => ({
  id,
  name,
  cwd,
  endpoint,
  herdr: herdrFlag,
  closedAt: 1,
  git: gitId,
});

test("closedProjectId is stable per endpoint+cwd, local when there is no endpoint", async () => {
  const { closedProjectId } = await library;
  assert.equal(
    closedProjectId(undefined, "/Users/dev/app"),
    "closed:local:/Users/dev/app",
  );
  assert.equal(
    closedProjectId("ssh:devbox", "/home/dev/app"),
    "closed:ssh:devbox:/home/dev/app",
  );
});

test('closedProjectId normalizes a local Herdr socket path to "local" (P2-f)', async () => {
  const { closedProjectId } = await library;
  assert.equal(
    closedProjectId("/tmp/sushiai-local.sock", "/Users/dev/app"),
    "closed:local:/Users/dev/app",
    "a local Herdr workspace's own socket is still this Mac, same as no endpoint",
  );
});

test("refreshProject updates an entry only while it is still on the list", async () => {
  const { refreshProject, forgetProject } = await library;
  const entry = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    false,
    git("", "/Users/dev/app"),
  );
  const distractor = closedProject(
    "closed:local:/Users/dev/other",
    "other",
    "/Users/dev/other",
    undefined,
    false,
    git("", "/Users/dev/other"),
  );
  const list = [entry, distractor];
  const refreshed = { ...entry, git: git("example.test/dev/app", entry.cwd) };
  const updated = refreshProject(list, refreshed);
  assert.equal(updated.length, 2);
  assert.equal(
    updated.find((p) => p.id === entry.id).git.remote,
    refreshed.git.remote,
  );

  // The user reopened or forgot the project while the git read was in flight.
  const gone = forgetProject(list, entry.id);
  assert.equal(
    refreshProject(gone, refreshed),
    gone,
    "an entry no longer on the list is never resurrected",
  );
});

test("rememberProject dedupes by id and keeps the freshest entry first", async () => {
  const { rememberProject } = await library;
  const first = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    false,
    git("", "/Users/dev/app"),
  );
  const other = closedProject(
    "closed:local:/Users/dev/other",
    "other",
    "/Users/dev/other",
    undefined,
    false,
    git("", "/Users/dev/other"),
  );
  const refreshed = { ...first, name: "renamed" };
  const list = rememberProject(rememberProject([], other), first);
  const next = rememberProject(list, refreshed);
  assert.equal(next.length, 2, "same id replaces, does not duplicate");
  assert.equal(next[0].name, "renamed");
  assert.equal(next[1].id, other.id);
});

test("forgetProject drops only the matching id", async () => {
  const { forgetProject } = await library;
  const a = closedProject("closed:local:/a", "a", "/a", undefined, false, {});
  const b = closedProject("closed:local:/b", "b", "/b", undefined, false, {});
  assert.deepEqual(forgetProject([a, b], a.id), [b]);
});

test("dashboardEntries: an open workspace and an unrelated closed project stay separate cards", async () => {
  const { dashboardEntries } = await library;
  const open = workspace("w-open", undefined, "/Users/dev/api", "api");
  const closed = closedProject(
    "closed:local:/Users/dev/tools",
    "tools",
    "/Users/dev/tools",
    undefined,
    false,
    git("", "/Users/dev/tools"),
  );
  const entries = dashboardEntries(
    [open],
    [closed],
    { "w-open": git("", open.cwd) },
    [],
  );
  assert.equal(entries.length, 2);
  const openEntry = entries.find((e) => e.id === "w-open");
  const closedEntry = entries.find((e) => e.id === closed.id);
  assert.equal(openEntry.closed, false);
  assert.equal(closedEntry.closed, true);
  assert.equal(closedEntry.primaryId, closed.id);
  assert.equal(closedEntry.panelCount, 0);
});

test("dashboardEntries: a closed project merges with an open workspace of the same project on another host", async () => {
  const { dashboardEntries } = await library;
  const openRemote = workspace(
    "w-lab",
    "ssh:lab",
    "/home/dev/app",
    "app on lab",
  );
  const closedLocal = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    true,
    git(REMOTE, "/Users/dev/app"),
  );
  // Distractor: closed on a *different* project, must never join the group.
  const distractor = closedProject(
    "closed:local:/Users/dev/other-app",
    "other-app",
    "/Users/dev/other-app",
    undefined,
    false,
    git("example.test/team/other-app", "/Users/dev/other-app"),
  );
  const entries = dashboardEntries(
    [openRemote],
    [closedLocal, distractor],
    { "w-lab": git(REMOTE, openRemote.cwd) },
    [profile("lab", "Lab")],
  );
  assert.equal(entries.length, 2, "the merged pair plus the distractor");
  const merged = entries.find((e) => e.members.length === 2);
  assert.ok(merged, "the closed and open members merged into one card");
  assert.equal(merged.closed, false, "one open member keeps the card open");
  assert.equal(merged.primaryId, "w-lab");
  assert.equal(merged.panelCount, 0);
  const closedMember = merged.members.find((m) => m.closed);
  assert.equal(closedMember.id, closedLocal.id);
  const standalone = entries.find((e) => e.members.length === 1);
  assert.equal(standalone.id, distractor.id);
});

test("dashboardEntries: a merged card keeps the main checkout's name while that checkout is closed", async () => {
  const { dashboardEntries } = await library;
  const feature = workspace("w-feature", undefined, "/Users/dev/app-feature-x");
  const featureGit = {
    ...git(REMOTE, "/Users/dev/app-feature-x", "feature-x"),
    commonDir: "/Users/dev/app/.git",
  };
  const main = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    false,
    git(REMOTE, "/Users/dev/app"),
  );
  const entries = dashboardEntries(
    [feature],
    [main],
    { "w-feature": featureGit },
    [],
  );
  assert.equal(entries.length, 1);
  assert.equal(
    entries[0].name,
    "app",
    "named like the sidebar row, not after the open worktree",
  );
  assert.equal(
    entries[0].primaryId,
    "w-feature",
    "the card still opens the member that is open",
  );
});

test("dashboardEntries: two hosts closing the same project merge into one fully-closed card", async () => {
  const { dashboardEntries } = await library;
  const closedLocal = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    true,
    git(REMOTE, "/Users/dev/app"),
  );
  const closedLab = closedProject(
    "closed:ssh:lab:/home/dev/app",
    "app",
    "/home/dev/app",
    "ssh:lab",
    true,
    git(REMOTE, "/home/dev/app"),
  );
  const entries = dashboardEntries([], [closedLocal, closedLab], {}, [
    profile("lab", "Lab"),
  ]);
  assert.equal(entries.length, 1);
  assert.equal(entries[0].closed, true);
  assert.equal(entries[0].members.length, 2);
  assert.ok(entries[0].members.every((m) => m.closed));
});

test("dashboardEntries: a closed project already reopened (same endpoint+cwd as an open workspace) is dropped", async () => {
  const { dashboardEntries } = await library;
  const open = workspace("w-open", undefined, "/Users/dev/app", "app");
  const stale = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    false,
    git("", "/Users/dev/app"),
  );
  const entries = dashboardEntries([open], [stale], {}, []);
  assert.equal(entries.length, 1);
  assert.equal(entries[0].id, "w-open");
});

test("dashboardEntries: a stale closed entry keyed on the local Herdr socket dedupes against the now-open workspace (P2-f)", async () => {
  const { dashboardEntries } = await library;
  const open = workspace(
    "w-open",
    "/tmp/sushiai-local.sock",
    "/Users/dev/app",
    "app",
  );
  const staleClosed = closedProject(
    // Old-style id from before the endpoint was normalized - restore must
    // still accept it even though closedProjectId no longer produces it.
    "closed:/tmp/sushiai-local.sock:/Users/dev/app",
    "app",
    "/Users/dev/app",
    "/tmp/sushiai-local.sock",
    true,
    git("", "/Users/dev/app"),
  );
  // Distractor: closed on the same local host but a different folder, must
  // still show up as its own card.
  const distractor = closedProject(
    "closed:local:/Users/dev/tools",
    "tools",
    "/Users/dev/tools",
    undefined,
    false,
    git("", "/Users/dev/tools"),
  );
  const entries = dashboardEntries(
    [open],
    [staleClosed, distractor],
    { "w-open": git("", open.cwd) },
    [],
  );
  assert.equal(entries.length, 2, "the reopened project plus the distractor");
  assert.ok(
    entries.every((e) => e.id !== staleClosed.id),
    "the stale closed duplicate never shows next to its now-open workspace",
  );
});

test("dashboardEntries: a closed project on a hidden SSH profile never appears", async () => {
  const { dashboardEntries } = await library;
  const closedHidden = closedProject(
    "closed:ssh:lab:/home/dev/app",
    "app",
    "/home/dev/app",
    "ssh:lab",
    true,
    git("", "/home/dev/app"),
  );
  const entries = dashboardEntries([], [closedHidden], {}, [
    profile("lab", "Lab", { hidden: true }),
  ]);
  assert.equal(entries.length, 0);
});

test("closedMemberIds: every closed member on a mixed card, every member on an all-closed one", async () => {
  const { dashboardEntries, closedMemberIds } = await library;
  const openRemote = workspace(
    "w-lab",
    "ssh:lab",
    "/home/dev/app",
    "app on lab",
  );
  const closedLocal = closedProject(
    "closed:local:/Users/dev/app",
    "app",
    "/Users/dev/app",
    undefined,
    true,
    git(REMOTE, "/Users/dev/app"),
  );
  const entries = dashboardEntries(
    [openRemote],
    [closedLocal],
    { "w-lab": git(REMOTE, openRemote.cwd) },
    [profile("lab", "Lab")],
  );
  const mixed = entries.find((e) => e.members.length === 2);
  assert.deepEqual(closedMemberIds(mixed), [closedLocal.id]);

  const closedLab = closedProject(
    "closed:ssh:lab:/home/dev/app",
    "app",
    "/home/dev/app",
    "ssh:lab",
    true,
    git(REMOTE, "/home/dev/app"),
  );
  const allClosed = dashboardEntries([], [closedLocal, closedLab], {}, [
    profile("lab", "Lab"),
  ])[0];
  assert.deepEqual(
    closedMemberIds(allClosed).sort(),
    [closedLocal.id, closedLab.id].sort(),
    "removing an all-closed card drops every member, not just primaryId",
  );
});
