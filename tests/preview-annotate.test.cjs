const test = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");
const { PreviewServer, withAnnotations } = require("../electron/preview.cjs");

const page = "<html><body><h2>Slide</h2><p>Hello</p></body></html>";

/** The "read" contract of remote-files.py: a whole file, or only the slice
 * asked for with `offset` and `length`, always with the size and version. */
function sliced(bytes, mime, version = "v1") {
  const calls = [];
  return {
    calls,
    inspect: async (_endpoint, options) => {
      calls.push(options);
      const from = options.offset ?? 0;
      const to =
        options.length === undefined ? bytes.length : from + options.length;
      return {
        base64: bytes.subarray(from, to).toString("base64"),
        size: bytes.length,
        version,
        mime,
      };
    },
  };
}

function fakeConnections(text) {
  return sliced(Buffer.from(text), "text/html");
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
  const server = new PreviewServer(sliced(bytes, "text/html"));
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

test("a page under a dot-folder opens, and its grant stays in the page's folder", async () => {
  const http = require("node:http");
  const { PreviewServer } = require("../electron/preview.cjs");
  const served = [];
  const inner = sliced(Buffer.from("<p>x</p>"), "text/html");
  const server = new PreviewServer({
    inspect: async (endpoint, options) => {
      if (options.length === 0) served.push(options.path);
      return inner.inspect(endpoint, options);
    },
  });
  await server.start();
  const get = (url) =>
    new Promise((resolve, reject) =>
      http
        .get(url, (res) => (res.resume(), resolve(res.statusCode)))
        .on("error", reject),
    );
  try {
    const url = server.grant(
      null,
      "/proj",
      ".claude/worktrees/x/artifacts/a.html",
    );
    assert.equal(await get(url), 200);
    const base = url.slice(0, url.indexOf("/.claude/"));
    assert.equal(await get(`${base}/.claude/worktrees/x/artifacts/b.css`), 200);
    assert.equal(
      await get(`${base}/.claude/worktrees/x/artifacts/.env.json`),
      403,
    );
    assert.equal(await get(`${base}/other/a.html`), 403);
    assert.deepEqual(served, [
      ".claude/worktrees/x/artifacts/a.html",
      ".claude/worktrees/x/artifacts/b.css",
    ]);
  } finally {
    server.server.close();
  }
});

test("a video is served with its type and byte ranges so the player can seek", async () => {
  const bytes = Buffer.from("0123456789");
  const server = new PreviewServer(sliced(bytes, "video/quicktime"));
  await server.start();
  try {
    const url = server.grant(undefined, "/r", "clip.mov");
    const whole = await fetch(url);
    assert.equal(whole.status, 200);
    assert.equal(whole.headers.get("accept-ranges"), "bytes");
    assert.equal(whole.headers.get("content-type"), "video/quicktime");
    const part = await fetch(url, { headers: { range: "bytes=2-4" } });
    assert.equal(part.status, 206);
    assert.equal(part.headers.get("content-range"), "bytes 2-4/10");
    assert.equal(await part.text(), "234");
    const tail = await fetch(url, { headers: { range: "bytes=7-" } });
    assert.equal(await tail.text(), "789");
    const last = await fetch(url, { headers: { range: "bytes=-3" } });
    assert.equal(await last.text(), "789");
    const bad = await fetch(url, { headers: { range: "bytes=50-60" } });
    assert.equal(bad.status, 416);
  } finally {
    server.close();
  }
});

test("a video carries an ETag, and a range made against another version gets the whole file", async () => {
  const bytes = Buffer.from("0123456789");
  const server = new PreviewServer(sliced(bytes, "video/mp4", "10-777"));
  await server.start();
  try {
    const url = server.grant(undefined, "/r", "clip.mp4");
    const whole = await fetch(url);
    assert.equal(whole.headers.get("etag"), '"10-777"');
    const part = await fetch(url, { headers: { range: "bytes=2-4" } });
    assert.equal(part.status, 206);
    assert.equal(part.headers.get("etag"), '"10-777"');
    const match = await fetch(url, {
      headers: { range: "bytes=2-4", "if-range": '"10-777"' },
    });
    assert.equal(match.status, 206);
    const stale = await fetch(url, {
      headers: { range: "bytes=2-4", "if-range": '"10-111"' },
    });
    assert.equal(stale.status, 200);
    assert.equal(await stale.text(), "0123456789");
  } finally {
    server.close();
  }
});

test("a range on a file larger than one read is a 206 with the right bytes, read by slice", async () => {
  const big = Buffer.alloc(20 * 1024 * 1024 + 5, 7);
  big.write("HEAD", 0);
  big.write("MID!", 17 * 1024 * 1024);
  big.write("TAIL", big.length - 4);
  const connections = sliced(big, "video/mp4");
  const server = new PreviewServer(connections);
  await server.start();
  try {
    const url = server.grant(undefined, "/r", "big.mp4");
    const at = 17 * 1024 * 1024;
    const part = await fetch(url, {
      headers: { range: `bytes=${at}-${at + 3}` },
    });
    assert.equal(part.status, 206);
    assert.equal(
      part.headers.get("content-range"),
      `bytes ${at}-${at + 3}/${big.length}`,
    );
    assert.equal(await part.text(), "MID!");
    const tail = await fetch(url, { headers: { range: "bytes=-4" } });
    assert.equal(await tail.text(), "TAIL");
    assert.ok(
      connections.calls.every(
        (call) => call.length !== undefined && call.length <= 8 * 1024 * 1024,
      ),
    );
    const all = await fetch(url);
    assert.equal(all.status, 200);
    assert.ok(Buffer.from(await all.arrayBuffer()).equals(big));
  } finally {
    server.close();
  }
});

test("the read operation returns only the slice asked for, with size and a cheap version", async (t) => {
  const fsp = require("node:fs/promises");
  const os = require("node:os");
  const { Connections } = require("../electron/connections.cjs");
  const base = await fsp.mkdtemp(path.join(os.tmpdir(), "preview-slice-"));
  await fsp.writeFile(path.join(base, "clip.mp4"), "0123456789");
  const connections = new Connections(base);
  await connections.init();
  t.after(async () => {
    await connections.close();
    await fsp.rm(base, { recursive: true, force: true });
  });
  const read = (extra) =>
    connections.inspect(null, {
      operation: "read",
      root: base,
      path: "clip.mp4",
      ...extra,
    });
  const part = await read({ offset: 3, length: 4 });
  assert.equal(Buffer.from(part.base64, "base64").toString(), "3456");
  assert.equal(part.size, 10);
  const stat = await read({ offset: 0, length: 0 });
  assert.equal(stat.base64, "");
  assert.equal(stat.version, part.version);
  const tail = await read({ offset: 8 });
  assert.equal(Buffer.from(tail.base64, "base64").toString(), "89");
  await fsp.writeFile(path.join(base, "clip.mp4"), "0123456789ab");
  assert.notEqual((await read({ offset: 0, length: 0 })).version, part.version);
  await assert.rejects(read({ offset: -1, length: 1 }), /Invalid offset/);
});
