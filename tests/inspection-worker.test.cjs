const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { EventEmitter } = require("node:events");
const { PassThrough } = require("node:stream");
const { Connections, run } = require("../electron/connections.cjs");
const { InspectionWorker } = require("../electron/inspection-worker.cjs");

async function fixture() {
  const root = await fs.mkdtemp("/tmp/sushiai-inspection-worker-");
  const connections = new Connections(root);
  await connections.init();
  return { root, connections };
}

test("inspection worker serializes concurrent calls and keeps ordinary errors recoverable", async (t) => {
  const { root, connections } = await fixture();
  t.after(async () => {
    await connections.close();
    await fs.rm(root, { recursive: true, force: true });
  });

  await fs.writeFile(path.join(root, "hello.txt"), "hello\n");
  const [home, listing, text] = await Promise.all([
    connections.inspect(null, { operation: "home", root }),
    connections.inspect(null, { operation: "list", root }),
    connections.inspect(null, { operation: "read", root, path: "hello.txt" }),
  ]);
  assert.equal(home.home, process.env.HOME);
  assert.ok(listing.entries.some((entry) => entry.name === "hello.txt"));
  assert.equal(Buffer.from(text.base64, "base64").toString(), "hello\n");
  assert.equal(connections.inspectionWorkers.size, 1);

  await assert.rejects(
    connections.inspect(null, { operation: "read", root, path: "../outside" }),
    /outside/,
  );
  assert.equal(
    (await connections.inspect(null, { operation: "home", root })).home,
    process.env.HOME,
  );
  assert.equal(connections.inspectionWorkers.size, 1);
});

test("inspection worker restarts after transport loss and closes cleanly", async (t) => {
  const { root, connections } = await fixture();
  t.after(async () => {
    await connections.close();
    await fs.rm(root, { recursive: true, force: true });
  });

  await connections.inspect(null, { operation: "home", root });
  const worker = connections.inspectionWorkers.get("local");
  const previous = worker.child;
  previous.kill();
  await new Promise((resolve) => previous.once("close", resolve));
  assert.equal(worker.child, null);

  await connections.inspect(null, { operation: "home", root });
  assert.ok(worker.child);
  assert.notStrictEqual(worker.child, previous);
  await connections.close();
  assert.equal(worker.closed, true);
  assert.equal(worker.child, null);
  await assert.rejects(
    connections.inspect(null, { operation: "home", root }),
    /closed/,
  );
});

test("remote-files keeps the one-shot JSON protocol compatible", async (t) => {
  const { root, connections } = await fixture();
  t.after(async () => {
    await connections.close();
    await fs.rm(root, { recursive: true, force: true });
  });
  const source = await fs.readFile(
    path.join(__dirname, "..", "electron", "remote-files.py"),
    "utf8",
  );
  const output = await run(
    "/usr/bin/python3",
    ["-c", source],
    JSON.stringify({ operation: "home", root }),
  );
  const envelope = JSON.parse(output);
  assert.equal(envelope.result.home, process.env.HOME);
});

test("inspection worker settles a startup failure instead of leaving a request pending", async () => {
  const worker = new InspectionWorker({
    command: "python3",
    args: () => [],
    sourceLoader: async () => "",
    spawnProcess: () => {
      throw new Error("spawn failed");
    },
  });
  await assert.rejects(worker.request({ operation: "home" }), /spawn failed/);
  await worker.close();
});

test("inspection worker rejects a matching response without a result or error", async () => {
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.stdin = new PassThrough();
  child.exitCode = null;
  child.signalCode = null;
  child.kill = () => {
    child.exitCode = 0;
    child.stdout.end();
    child.stderr.end();
    child.stdin.end();
    child.emit("close", 0);
  };
  child.stdin.on("data", () => child.stdout.write('{"id":"1"}\n'));
  const worker = new InspectionWorker({
    command: "fake",
    args: () => [],
    sourceLoader: async () => "",
    spawnProcess: () => child,
  });
  await assert.rejects(
    worker.request({ operation: "home" }),
    /invalid response/,
  );
  await worker.close();
});

function fakeChild(onFrame) {
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.stdin = new PassThrough();
  child.exitCode = null;
  child.signalCode = null;
  child.kill = () => {
    if (child.exitCode !== null) return;
    child.exitCode = 0;
    child.stdout.end();
    child.stderr.end();
    child.emit("close", 0);
  };
  let rest = "";
  child.stdin.on("data", (chunk) => {
    rest += chunk;
    let newline;
    while ((newline = rest.indexOf("\n")) >= 0) {
      const line = rest.slice(0, newline);
      rest = rest.slice(newline + 1);
      onFrame(JSON.parse(line), child);
    }
  });
  return child;
}

test("a remote shell banner ahead of the first frame is tolerated", async () => {
  const child = fakeChild((frame, target) =>
    target.stdout.write(`${JSON.stringify({ id: frame.id, result: "ok" })}\n`),
  );
  const worker = new InspectionWorker({
    command: "fake",
    args: () => [],
    sourceLoader: async () => "",
    spawnProcess: () => {
      // Motd, profile output and login banners all land before python speaks.
      queueMicrotask(() =>
        child.stdout.write("Welcome to example.test\nLast login: today\n"),
      );
      return child;
    },
  });
  assert.equal(await worker.request({ operation: "home" }), "ok");
  await worker.close();
});

test("garbage after the stream is framed names the shell startup output", async () => {
  let seen = 0;
  const child = fakeChild((frame, target) => {
    seen += 1;
    if (seen === 1)
      target.stdout.write(
        `${JSON.stringify({ id: frame.id, result: "ok" })}\n`,
      );
    else target.stdout.write("this is not json\n");
  });
  const worker = new InspectionWorker({
    command: "fake",
    args: () => [],
    sourceLoader: async () => "",
    spawnProcess: () => {
      queueMicrotask(() => child.stdout.write("MOTD banner line\n"));
      return child;
    },
  });
  assert.equal(await worker.request({ operation: "home" }), "ok");
  await assert.rejects(worker.request({ operation: "home" }), /malformed data/);
  await worker.close();
});

test("a timed-out request does not cancel the ones queued behind it", async () => {
  const answered = [];
  const child = fakeChild((frame, target) => {
    // The first request never answers; every later one does.
    if (answered.length === 0) {
      answered.push(frame.id);
      return;
    }
    answered.push(frame.id);
    target.stdout.write(
      `${JSON.stringify({ id: frame.id, result: frame.id })}\n`,
    );
  });
  let spawns = 0;
  const worker = new InspectionWorker({
    command: "fake",
    args: () => [],
    sourceLoader: async () => "",
    spawnProcess: () => {
      spawns += 1;
      return spawns === 1
        ? child
        : fakeChild((frame, target) =>
            target.stdout.write(
              `${JSON.stringify({ id: frame.id, result: frame.id })}\n`,
            ),
          );
    },
  });

  const stuck = worker.request({ operation: "home" });
  const queued = worker.request({ operation: "home" });
  // Expire the in-flight request the way its own timer would.
  await new Promise((resolve) => setImmediate(resolve));
  worker.expire(worker.active);

  await assert.rejects(stuck, /timed out/);
  assert.equal(
    await queued,
    "2",
    "the queued request is replayed against the replacement worker",
  );
  assert.equal(spawns, 2, "the stuck worker is replaced, not reused");
  await worker.close();
});

test("the python worker survives an exception its catch list never named", async () => {
  const { root, connections } = await fixture();
  try {
    const source = await fs.readFile(
      path.join(__dirname, "..", "electron", "remote-files.py"),
      "utf8",
    );
    // AttributeError is outside the original (OSError, ValueError, KeyError,
    // TimeoutExpired, TypeError) list, so it used to kill the shared worker.
    connections.inspectionSource = async () =>
      source.replace(
        "def inspect(data):",
        'def inspect(data):\n    if data.get("operation") == "boom":\n        raise AttributeError("exploded")',
      );

    await assert.rejects(
      connections.inspect(null, { operation: "boom", root }),
      /exploded/,
      "the failure is reported for that request",
    );
    const home = await connections.inspect(null, { operation: "home" });
    assert.ok(home, "the worker still answers the next request");
  } finally {
    await connections.close();
    await fs.rm(root, { recursive: true, force: true });
  }
});
