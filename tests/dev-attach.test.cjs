const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");

const root = path.resolve(__dirname, "..");
const attach = path.join(root, ".agents/skills/ui-evidence/scripts/attach.mjs");
const electronBin = path.join(root, "node_modules/.bin/electron");

test("the debugging flag exists only in scripts/dev.mjs", () => {
  const walk = (dir) =>
    fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
      const full = path.join(dir, entry.name);
      return entry.isDirectory() ? walk(full) : [full];
    });
  const shipped = [
    ...walk(path.join(root, "electron")),
    path.join(root, "package.json"),
  ];
  for (const file of shipped)
    assert.ok(
      !/remote-debugging/.test(fs.readFileSync(file, "utf8")),
      `${path.relative(root, file)} must not open a debugging port`,
    );
  const packaged = JSON.parse(
    fs.readFileSync(path.join(root, "package.json"), "utf8"),
  ).build;
  assert.ok(!JSON.stringify(packaged).includes("remote-debugging"));
});

test("dev publishes devtools.json and attach.mjs shoots and measures the live window", async () => {
  const { DEVTOOLS_FLAGS, publishDevtools, parseListening, readDevtools } =
    await import(path.join(root, "scripts/lib/devtools-endpoint.mjs"));
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-attach-"));
  const app = path.join(tmp, "app");
  fs.mkdirSync(app);
  fs.writeFileSync(
    path.join(app, "package.json"),
    JSON.stringify({ name: "fixture", main: "main.cjs" }),
  );
  fs.writeFileSync(
    path.join(app, "main.cjs"),
    `const { app, BrowserWindow } = require("electron");
app.setPath("userData", process.env.FIXTURE_DATA);
app.whenReady().then(() => {
  const win = new BrowserWindow({ show: false, width: 640, height: 480 });
  win.loadURL("data:text/html," + encodeURIComponent(
    '<body style="margin:0"><div id="box" style="width:120px;height:40px;background:rgb(1,2,3)">hi</div></body>'));
});
`,
  );
  const home = path.join(tmp, "home");
  fs.mkdirSync(home);
  // SUSHIAI_TEST_WINDOW=hidden: the fixture window never shows anyway.
  const child = spawn(electronBin, [...DEVTOOLS_FLAGS, app], {
    stdio: ["ignore", "ignore", "pipe"],
    env: {
      ...process.env,
      SUSHIAI_TEST_WINDOW: "hidden",
      FIXTURE_DATA: path.join(tmp, "data"),
    },
  });
  const exited = new Promise((resolve) => child.on("exit", resolve));
  publishDevtools(child, home);
  const file = path.join(home, "devtools.json");
  try {
    // publishDevtools listens first, so the file exists when this resolves.
    await new Promise((resolve, reject) => {
      child.stderr.on("data", (chunk) => {
        if (parseListening(chunk.toString())) resolve();
      });
      child.on("exit", () => reject(new Error("fixture exited early")));
    });
    const info = readDevtools(home);
    assert.equal(info.pid, child.pid);
    assert.ok(info.port > 0);
    assert.match(info.wsEndpoint, /^ws:\/\/127\.0\.0\.1:\d+\//);
    assert.equal(fs.statSync(file).mode & 0o777, 0o600);

    const env = { ...process.env, HOME: tmp, SUSHIAI_HOME: home };
    const run = (...args) => {
      const result = spawnSync(process.execPath, [attach, ...args], {
        env,
        encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout;
    };
    const png = path.join(tmp, "out.png");
    run("shot", png);
    assert.equal(fs.readFileSync(png).subarray(1, 4).toString(), "PNG");
    const [measured] = JSON.parse(run("measure", "#box"));
    assert.deepEqual(
      [measured.matches[0].box.width, measured.matches[0].box.height],
      [120, 40],
    );
    assert.equal(measured.matches[0].styles.backgroundColor, "rgb(1, 2, 3)");
    assert.equal(JSON.parse(run("eval", "1 + 1")), 2);
  } finally {
    child.kill();
    await exited;
    assert.ok(!fs.existsSync(file), "devtools.json is deleted on exit");
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
