const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { EventEmitter } = require("node:events");

const root = path.resolve(__dirname, "..");
const lib = () => import(path.join(root, "scripts/lib/devtools-endpoint.mjs"));
const attach = () =>
  import(path.join(root, ".agents/skills/ui-evidence/scripts/attach.mjs"));

const LINE =
  "DevTools listening on ws://127.0.0.1:41234/devtools/browser/abc-123";

test("parseListening reads the port and endpoint from Chromium's line", async () => {
  const { parseListening } = await lib();
  assert.deepEqual(parseListening(LINE), {
    wsEndpoint: "ws://127.0.0.1:41234/devtools/browser/abc-123",
    port: 41234,
  });
  assert.equal(parseListening("DevTools listening on ws://0.0.0.0:1/x"), null);
  assert.equal(parseListening("something else"), null);
});

test("publishDevtools waits for a whole line and removes the file on exit", async () => {
  const { publishDevtools } = await lib();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-devtools-"));
  const file = path.join(dir, "nested", "devtools.json");
  const child = new EventEmitter();
  child.stderr = new EventEmitter();
  child.pid = 4242;
  publishDevtools(child, file);
  try {
    child.stderr.emit("data", Buffer.from(LINE.slice(0, 30)));
    assert.ok(!fs.existsSync(file), "a half line publishes nothing");
    child.stderr.emit("data", Buffer.from(LINE.slice(30) + "\n"));
    const info = JSON.parse(fs.readFileSync(file, "utf8"));
    assert.deepEqual(info, {
      wsEndpoint: "ws://127.0.0.1:41234/devtools/browser/abc-123",
      port: 41234,
      pid: 4242,
    });
    assert.equal(fs.statSync(file).mode & 0o777, 0o600);
    child.emit("exit");
    assert.ok(!fs.existsSync(file));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("readDevtools rejects a missing file and any failed pid probe", async () => {
  const { readDevtools } = await lib();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-devtools-"));
  const file = path.join(dir, "devtools.json");
  try {
    assert.throws(() => readDevtools(file), /missing/);
    fs.writeFileSync(
      file,
      JSON.stringify({ port: 1, wsEndpoint: "ws://x", pid: 7 }),
    );
    const failing = (code) => () => {
      throw Object.assign(new Error(code), { code });
    };
    assert.throws(() => readDevtools(file, failing("ESRCH")), /stale/);
    assert.throws(() => readDevtools(file, failing("EPERM")), /stale/);
    assert.equal(readDevtools(file, () => true).pid, 7);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("attach is on unless SUSHIAI_DEV_ATTACH=0", async () => {
  const { attachEnabled } = await lib();
  assert.equal(attachEnabled({}), true);
  assert.equal(attachEnabled({ SUSHIAI_DEV_ATTACH: "1" }), true);
  assert.equal(attachEnabled({ SUSHIAI_DEV_ATTACH: "0" }), false);
});

test("pickMainPage takes only the page at the dev URL root, visible first", async () => {
  const { pickMainPage } = await attach();
  const dev = "http://127.0.0.1:5173";
  const pages = [
    {
      id: "devtools",
      url: "devtools://devtools/bundled/inspector.html",
      visible: true,
    },
    { id: "mascot", url: `${dev}/mascot.html`, visible: true },
    { id: "other-origin", url: "http://127.0.0.1:9999/", visible: true },
    { id: "blank", url: "about:blank", visible: true },
    { id: "hidden-main", url: `${dev}/`, visible: false },
    { id: "main", url: `${dev}/index.html`, visible: true },
  ];
  assert.equal(pickMainPage(pages, dev).id, "main");
  assert.equal(
    pickMainPage(
      pages.filter((p) => p.id !== "main"),
      dev,
    ).id,
    "hidden-main",
  );
  assert.equal(pickMainPage(pages.slice(0, 4), dev), null);
});
