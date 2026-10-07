const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { spawnPipe } = require("../electron/daemon/connectors.cjs");
const { createCommandConnector } = require("../electron/daemon/command.cjs");
const {
  describeExit,
} = require("../electron/extensions/companion-process.cjs");

test("spawnPipe: exited resolves when a grandchild keeps the stdio open", async () => {
  const pipe = spawnPipe("/bin/sh", ["-c", "sleep 3 & exit 3"]);
  pipe.on("error", () => {});
  const started = Date.now();
  const exit = await pipe.exited;
  assert.equal(exit.code, 3);
  assert.ok(Date.now() - started < 2000, "exited waited for the grandchild");
});

test("command connector hands the program an allowlisted environment", () => {
  const seen = [];
  const secret = "SUSHIAI_TEST_SECRET_FOR_ENV";
  process.env[secret] = "x";
  try {
    createCommandConnector({
      profile: { connector: { kind: "command", argv: ["prog", "--stdio"] } },
      spawnProcess: (command, args, options) => {
        seen.push(options);
        return spawnPipe("/bin/sh", ["-c", "exit 0"]);
      },
    })
      .connect()
      .catch(() => {});
  } finally {
    delete process.env[secret];
  }
  assert.equal(seen.length, 1);
  assert.equal(seen[0].env[secret], undefined);
  assert.equal(seen[0].env.PATH, process.env.PATH);
});

test("a companion failure reads as a companion, not as a connector", () => {
  assert.equal(describeExit({ code: 2 }), "The companion exited with 2.");
  assert.equal(
    describeExit({ signal: "SIGKILL" }),
    "The companion ended with SIGKILL.",
  );
});

test("main starts the orchestrator and companions without blocking the window", () => {
  const source = fs.readFileSync(
    path.join(__dirname, "../electron/main.cjs"),
    "utf8",
  );
  const window = source.indexOf("new BrowserWindow({\n    ...startBounds");
  assert.ok(window > 0);
  const before = source.slice(0, window);
  assert.doesNotMatch(before, /await orchestrator\.start\(\)/);
  assert.doesNotMatch(before, /await companions\.start\(\)/);
  assert.match(before, /void orchestrator\s*\.start\(\)/);
  assert.match(before, /\.then\(\(\) => companions\.start\(\)\)/);
});

test("Orchestrator starting switched off is declared by the registration", () => {
  const {
    ExtensionManager,
  } = require("../electron/extensions/extension-manager.cjs");
  const {
    ORCHESTRATOR_MANIFEST,
  } = require("../electron/extensions/builtin-orchestrator.cjs");
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-off-"));
  try {
    const plain = new ExtensionManager({
      dataDir,
      builtins: [ORCHESTRATOR_MANIFEST],
    });
    assert.equal(plain.isEnabled(ORCHESTRATOR_MANIFEST.id), true);
    const off = new ExtensionManager({
      dataDir,
      builtins: [ORCHESTRATOR_MANIFEST],
      builtinsStartOff: [ORCHESTRATOR_MANIFEST.id],
    });
    assert.equal(off.isEnabled(ORCHESTRATOR_MANIFEST.id), false);
  } finally {
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
});
