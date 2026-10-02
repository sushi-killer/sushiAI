const test = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");
const { PreviewServer, withAnnotations } = require("../electron/preview.cjs");

const page = "<html><body><h2>Slide</h2><p>Hello</p></body></html>";

function fakeConnections(text) {
  return {
    inspect: async () => ({
      base64: Buffer.from(text).toString("base64"),
      mime: "text/html",
    }),
  };
}

test("the comment script goes right before </body>, or at the end without one", () => {
  const out = withAnnotations(page);
  assert.ok(out.indexOf("<script>") < out.indexOf("</body>"));
  assert.ok(out.endsWith("</body></html>"));
  assert.ok(withAnnotations("<p>bare</p>").startsWith("<p>bare</p><script>"));
});

test("only an annotate grant injects the script, and only into HTML", async () => {
  const server = new PreviewServer(fakeConnections(page));
  await server.start();
  try {
    const plain = await (
      await fetch(server.grant(undefined, "/r", "a.html"))
    ).text();
    assert.equal(plain, page);
    const annotated = server.grant(undefined, "/r", "a.html", {
      annotate: true,
    });
    const body = await (await fetch(annotated)).text();
    assert.match(body, /sushiai: "annotate"/);
    const response = await fetch(annotated);
    assert.equal(
      Number(response.headers.get("content-length")),
      Buffer.byteLength(await response.text()),
    );
  } finally {
    server.close();
  }
});

test("the script posts a text selection with its quote and nearest heading", () => {
  const source = fs.readFileSync(
    path.join(__dirname, "../electron/preview-annotate.js"),
    "utf8",
  );
  const listeners = {};
  const posted = [];
  const heading = {
    nodeType: 1,
    textContent: "Results",
    matches: (s) => s.startsWith("h1"),
    previousElementSibling: null,
    parentElement: null,
  };
  const paragraph = {
    nodeType: 1,
    matches: () => false,
    querySelector: () => null,
    previousElementSibling: heading,
    parentElement: null,
  };
  const context = {
    location: { hash: "" },
    parent: { postMessage: (m) => posted.push(m) },
    addEventListener: () => {},
    document: { addEventListener: (type, fn) => (listeners[type] = fn) },
    getSelection: () => ({
      toString: () => "  the quoted words ",
      rangeCount: 1,
      anchorNode: paragraph,
      getRangeAt: () => ({
        getBoundingClientRect: () => ({ left: 1, top: 2, width: 3, height: 4 }),
      }),
    }),
  };
  vm.runInNewContext(source, context);
  listeners.mouseup();
  assert.deepEqual(JSON.parse(JSON.stringify(posted)), [
    {
      sushiai: "annotate",
      kind: "text",
      quote: "the quoted words",
      where: "Results",
      rect: { x: 1, y: 2, w: 3, h: 4 },
    },
  ]);
});

const CSP = "sandbox allow-scripts; connect-src 'none'; frame-src 'none'";

test("an annotated page cannot call back to the server; other grants are untouched", async () => {
  const server = new PreviewServer(fakeConnections(page));
  await server.start();
  try {
    const annotated = await fetch(
      server.grant(undefined, "/r", "a.html", { annotate: true }),
    );
    assert.equal(annotated.headers.get("content-security-policy"), CSP);
    const plain = await fetch(server.grant(undefined, "/r", "a.html"));
    assert.equal(plain.headers.get("content-security-policy"), null);
  } finally {
    server.close();
  }
});

test("a page that is not valid UTF-8 is served byte for byte, without the script", async () => {
  const bytes = Buffer.concat([
    Buffer.from("<html><body>caf"),
    Buffer.from([0xe9]),
    Buffer.from("</body></html>"),
  ]);
  const server = new PreviewServer({
    inspect: async () => ({
      base64: bytes.toString("base64"),
      mime: "text/html",
    }),
  });
  await server.start();
  try {
    const response = await fetch(
      server.grant(undefined, "/r", "a.html", { annotate: true }),
    );
    assert.deepEqual(Buffer.from(await response.arrayBuffer()), bytes);
    assert.equal(response.headers.get("content-security-policy"), CSP);
  } finally {
    server.close();
  }
});

test("a nested relative file and its sibling assets are served; .., absolute and symlink escapes are refused", async (t) => {
  const fsp = require("node:fs/promises");
  const os = require("node:os");
  const { Connections } = require("../electron/connections.cjs");
  const base = await fsp.mkdtemp(path.join(os.tmpdir(), "preview-nested-"));
  const project = path.join(base, "project");
  const outside = path.join(base, "outside.txt");
  await fsp.mkdir(path.join(project, "artifacts", "deck"), {
    recursive: true,
  });
  await fsp.writeFile(outside, "secret");
  await fsp.writeFile(
    path.join(project, "artifacts", "deck", "x.html"),
    "<html><body>deck</body></html>",
  );
  await fsp.writeFile(
    path.join(project, "artifacts", "deck", "style.css"),
    "body{}",
  );
  await fsp.symlink(outside, path.join(project, "artifacts", "leak.txt"));
  const connections = new Connections(base);
  await connections.init();
  const server = new PreviewServer(connections);
  await server.start();
  t.after(async () => {
    server.close();
    await connections.close();
    await fsp.rm(base, { recursive: true, force: true });
  });
  const url = server.grant(null, project, "artifacts/deck/x.html", {
    annotate: true,
  });
  assert.match(url, /\/artifacts\/deck\/x\.html$/);
  const page = await fetch(url);
  assert.equal(page.status, 200);
  assert.match(await page.text(), /deck/);
  const css = await fetch(new URL("style.css", url));
  assert.equal(css.status, 200);
  assert.equal(await css.text(), "body{}");
  for (const bad of ["../outside.txt", "/etc/hosts", "a/../b.html", ""])
    assert.throws(() => server.grant(null, project, bad), /inside the project/);
  const leak = server.grant(null, project, "artifacts/leak.txt");
  assert.equal((await fetch(leak)).status, 404);
});
