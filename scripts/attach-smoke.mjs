// Desktop lane: a real Electron started with the dev debugging flags publishes
// its endpoint, and attach.mjs shoots and measures it. Needs a display
// (macOS CI job), so it is not a tests/ file.
import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import {
  DEVTOOLS_FLAGS,
  parseListening,
  publishDevtools,
  readDevtools,
} from "./lib/devtools-endpoint.mjs";

const root = path.resolve(import.meta.dirname, "..");
const attach = path.join(root, ".agents/skills/ui-evidence/scripts/attach.mjs");
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
  win.loadURL(process.env.BRIDGE_DEV_URL + "/");
});
`,
);
// The fixture page stands in for the dev server's root page.
const server = http.createServer((request, response) => {
  response.setHeader("content-type", "text/html");
  response.end(
    '<body style="margin:0"><div id="box" style="width:120px;height:40px;background:rgb(1,2,3)">hi</div><input id="in" value="a"></body>',
  );
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const devUrl = `http://127.0.0.1:${server.address().port}`;
const file = path.join(tmp, "devtools.json");
const child = spawn(
  path.join(root, "node_modules/.bin/electron"),
  [...DEVTOOLS_FLAGS, app],
  {
    stdio: ["ignore", "ignore", "pipe"],
    env: {
      ...process.env,
      HOME: tmp,
      SUSHIAI_TEST_WINDOW: "hidden",
      FIXTURE_DATA: path.join(tmp, "data"),
      BRIDGE_DEV_URL: devUrl,
    },
  },
);
const exited = new Promise((resolve) => child.on("exit", resolve));
publishDevtools(child, file);
try {
  // publishDevtools listens first: the file exists once this resolves.
  await new Promise((resolve, reject) => {
    let seen = "";
    child.stderr.on("data", (chunk) => {
      seen += chunk;
      if (
        seen
          .split("\n")
          .slice(0, -1)
          .some((line) => parseListening(line))
      )
        resolve();
    });
    child.on("exit", () => reject(new Error("fixture exited early")));
  });
  const info = readDevtools(file);
  assert.equal(info.pid, child.pid);
  assert.equal(fs.statSync(file).mode & 0o777, 0o600);

  // Async, so this process keeps serving the fixture page while attach runs.
  const run = async (...args) => {
    const { stdout } = await promisify(execFile)(
      process.execPath,
      [attach, ...args],
      {
        env: {
          ...process.env,
          HOME: tmp,
          SUSHIAI_DEVTOOLS_FILE: file,
          BRIDGE_DEV_URL: devUrl,
        },
      },
    );
    return JSON.parse(stdout);
  };
  const png = path.join(tmp, "out.png");
  await run("shot", png);
  assert.equal(fs.readFileSync(png).subarray(1, 4).toString(), "PNG");
  const [box] = await run("measure", "#box");
  assert.deepEqual(
    [box.matches[0].box.width, box.matches[0].box.height],
    [120, 40],
  );
  assert.equal(box.matches[0].styles.backgroundColor, "rgb(1, 2, 3)");
  await run("type", "#in", "bc");
  assert.equal(await run("eval", 'document.querySelector("#in").value'), "abc");
  console.log("attach smoke ok");
} finally {
  child.kill();
  await exited;
  server.close();
  assert.ok(!fs.existsSync(file), "devtools.json is deleted on exit");
  fs.rmSync(tmp, { recursive: true, force: true });
}
