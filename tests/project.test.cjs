const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const {
  Connections,
  validate,
  quote,
  run,
} = require("../electron/connections.cjs");
const { PreviewServer } = require("../electron/preview.cjs");
const { detectAgent } = require("../electron/terminal-stream.cjs");
test("SSH validation and POSIX quoting", () => {
  assert.throws(() =>
    validate({ host: "-oProxyCommand=bad", socket: "/tmp/sock" }),
  );
  assert.throws(() => validate({ host: "lab;echo bad", socket: "/tmp/sock" }));
  assert.throws(() => validate({ host: "lab", socket: "../sock" }));
  assert.throws(() =>
    validate({ host: "lab", socket: "/tmp/sock", port: 70000 }),
  );
  assert.equal(quote("a'b $HOME"), "'a'\\''b $HOME'");
});
test("harness names are recognized without substring false positives", () => {
  assert.equal(detectAgent("/opt/bin/claude"), "claude");
  assert.equal(detectAgent("node", "codex — project"), "codex");
  assert.equal(detectAgent("zsh", "gemini"), "gemini");
  assert.equal(detectAgent("zsh", "my-codex-project"), null);
});
test("project inspection: text, images, git, containment and static preview", async () => {
  const root = await fs.mkdtemp("/tmp/sushiai-project-test-");
  const c = new Connections(root);
  await c.init();
  const preview = new PreviewServer(c);
  try {
    await fs.writeFile(path.join(root, "hello.txt"), "Hello sushiAI 🍣\n");
    await fs.writeFile(
      path.join(root, "index.html"),
      '<link rel="stylesheet" href="style.css"><h1>DEMO</h1>',
    );
    await fs.writeFile(path.join(root, "style.css"), "body{color:coral}");
    await fs.writeFile(path.join(root, ".env"), "TEST_ONLY=private");
    await fs.symlink("/etc", path.join(root, "outside"));
    const inspect = (operation, extra = {}) =>
      c.inspect(null, { operation, root, ...extra });
    const listing = await inspect("list");
    assert.ok(listing.entries.some((e) => e.name === "hello.txt"));
    assert.ok(
      !listing.entries.some((e) => [".env", "outside"].includes(e.name)),
    );
    assert.equal(
      Buffer.from(
        (await inspect("read", { path: "hello.txt" })).base64,
        "base64",
      ).toString(),
      "Hello sushiAI 🍣\n",
    );
    const original = await inspect("read", { path: "hello.txt" });
    await inspect("write", {
      path: "hello.txt",
      expectedHash: original.hash,
      text: "Saved from editor\n",
    });
    assert.equal(
      await fs.readFile(path.join(root, "hello.txt"), "utf8"),
      "Saved from editor\n",
    );
    await assert.rejects(
      inspect("write", {
        path: "hello.txt",
        expectedHash: original.hash,
        text: "stale",
      }),
      /changed on disk/,
    );
    await fs.writeFile(path.join(root, "hello.txt"), "Hello sushiAI 🍣\n");
    await assert.rejects(inspect("read", { path: "../other" }), /outside/);
    await assert.rejects(
      inspect("read", { path: "outside/passwd" }),
      /outside/,
    );
    await run("/usr/bin/git", ["init", root]);
    assert.ok((await inspect("git")).branch);
    await run("/usr/bin/git", ["-C", root, "add", "hello.txt"]);
    await run("/usr/bin/git", [
      "-C",
      root,
      "-c",
      "user.name=Test",
      "-c",
      "user.email=test@example.invalid",
      "commit",
      "-m",
      "fixture",
    ]);
    await fs.writeFile(path.join(root, "hello.txt"), "New line\n");
    assert.match(
      (await inspect("diff", { path: "hello.txt" })).text,
      /\+New line/,
    );
    await fs.rm(path.join(root, "hello.txt"));
    assert.match(
      (await inspect("diff", { path: "hello.txt" })).text,
      /-Hello sushiAI 🍣/,
    );
    await preview.start();
    const url = preview.grant(null, root, "index.html");
    assert.match(await (await fetch(url)).text(), /DEMO/);
    assert.match(
      await (await fetch(new URL("style.css", url))).text(),
      /coral/,
    );
    assert.equal((await fetch(new URL(".env", url))).status, 403);
    assert.equal((await fetch(new URL("outside/passwd", url))).status, 403);
    assert.equal((await fetch(url, { method: "POST" })).status, 405);
  } finally {
    preview.close();
    await c.close();
    await fs.rm(root, { recursive: true, force: true });
  }
});
