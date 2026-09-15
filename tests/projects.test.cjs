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
