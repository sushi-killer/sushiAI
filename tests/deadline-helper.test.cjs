const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const {
  DEADLINE_MS,
  deadlineMs,
  until,
  pidStart,
  stopPid,
  removeDir,
} = require("./helpers/deadline.cjs");

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

async function spawnReady(script) {
  const child = spawn(process.execPath, ["-e", script], {
    stdio: ["ignore", "pipe", "ignore"],
  });
  await new Promise((resolve) => child.stdout.once("data", resolve));
  return child;
}

test("deadline defaults to 180 s and takes a positive env override", () => {
  assert.equal(deadlineMs({}), 180000);
  assert.equal(deadlineMs({ SUSHIAI_TEST_DEADLINE_SECS: "5" }), 5000);
  assert.equal(deadlineMs({ SUSHIAI_TEST_DEADLINE_SECS: "abc" }), 180000);
  assert.equal(deadlineMs({ SUSHIAI_TEST_DEADLINE_SECS: "0" }), 180000);
  assert.equal(deadlineMs({ SUSHIAI_TEST_DEADLINE_SECS: "-3" }), 180000);
  assert.equal(deadlineMs({ SUSHIAI_TEST_DEADLINE_SECS: "" }), 180000);
  assert.equal(typeof DEADLINE_MS, "number");
});

test("until returns the first truthy value", async () => {
  let calls = 0;
  const value = await until(async () => (++calls >= 3 ? "ready" : null), "x");
  assert.equal(value, "ready");
  assert.equal(calls, 3);
});

test("until throws a named timeout after the deadline", async () => {
  await assert.rejects(
    until(() => false, "the thing", { deadlineMs: 100 }),
    { message: "timed out waiting for the thing" },
  );
});

test("stopPid ends a child that exits on SIGTERM", async () => {
  const child = await spawnReady("console.log('up');setInterval(()=>{},1000)");
  await stopPid(child.pid);
  assert.equal(alive(child.pid), false);
});

test("stopPid falls back to SIGKILL for a child that ignores SIGTERM", async () => {
  const child = await spawnReady(
    "process.on('SIGTERM',()=>{});console.log('up');setInterval(()=>{},1000)",
  );
  await stopPid(child.pid, { deadlineMs: 300 });
  assert.equal(alive(child.pid), false);
});

test("stopPid returns for a pid that is already gone", async () => {
  const child = await spawnReady("console.log('up')");
  await new Promise((resolve) => child.once("exit", resolve));
  await stopPid(child.pid, { deadlineMs: 300 });
});

test("removeDir removes a dir another process keeps writing into", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "deadline-rm-"));
  const child = await spawnReady(
    `const fs=require('fs'),p=require('path');let i=0;console.log('up');` +
      `const t=setInterval(()=>{try{fs.writeFileSync(p.join(${JSON.stringify(dir)},'f'+(i++)),'x')}catch{}},5);` +
      `setTimeout(()=>{clearInterval(t);process.exit(0)},600)`,
  );
  const exited = new Promise((resolve) => child.once("exit", resolve));
  removeDir(dir);
  await exited;
  assert.equal(fs.existsSync(dir), false);
});

test("stopPid leaves a pid alone when its start time differs", async () => {
  const child = await spawnReady("console.log('up');setInterval(()=>{},1000)");
  try {
    await stopPid(child.pid, { start: "not the start time" });
    assert.equal(alive(child.pid), true);
    await stopPid(child.pid, { start: pidStart(child.pid) });
    assert.equal(alive(child.pid), false);
  } finally {
    child.kill("SIGKILL");
  }
});

test("native pid probe returns a stable identity and an empty result after exit", async () => {
  const child = await spawnReady("console.log('up');setInterval(()=>{},1000)");
  try {
    const start = pidStart(child.pid);
    assert.ok(start);
    assert.equal(pidStart(child.pid), start);
    await stopPid(child.pid, { start });
    assert.equal(pidStart(child.pid), "");
  } finally {
    child.kill("SIGKILL");
  }
});
