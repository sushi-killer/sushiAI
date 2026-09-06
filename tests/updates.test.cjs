const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { createHash } = require("node:crypto");
const {
  Updates,
  compareVersions,
  selectRelease,
} = require("../electron/updates.cjs");
const bytes = Buffer.from("synthetic DMG fixture");
const sha = createHash("sha256").update(bytes).digest("hex");
function release(tag = "v0.0.2", arch = "arm64", extra = {}) {
  const name = `sushiAI-${tag.replace(/^v/, "")}-${arch}.dmg`;
  return {
    tag_name: tag,
    draft: false,
    prerelease: tag.includes("-"),
    body: "Release notes",
    assets: [
      {
        name,
        state: "uploaded",
        size: bytes.length,
        digest: `sha256:${sha}`,
        browser_download_url: `https://github.com/sushi-killer/sushiAI/releases/download/${tag}/${name}`,
      },
    ],
    ...extra,
  };
}
async function fixture(t, fetcher) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-updates-"));
  const opened = [];
  const updater = new Updates({
    directory: dir,
    currentVersion: "0.0.1-alpha.1",
    arch: "arm64",
    fetcher,
    automatic: false,
    openPath: async (p) => {
      opened.push(p);
      return "";
    },
    openExternal: async (url) => opened.push(url),
  });
  await updater.init();
  t.after(async () => {
    await updater.close();
    await fs.rm(dir, { recursive: true, force: true });
  });
  return { updater, dir, opened };
}
test("release selection uses semantic ordering, channel and architecture, never drafts or downgrades", () => {
  assert.equal(compareVersions("0.0.2-alpha.10", "0.0.2-alpha.2"), 1);
  assert.equal(compareVersions("v0.0.2", "0.0.2-rc.1"), 1);
  assert.equal(compareVersions("0.0.2+build", "0.0.2"), 0);
  const releases = [
    release("v0.0.2-alpha.2"),
    release("v0.0.2-alpha.10"),
    release("v0.0.3", "x64"),
    release("v9.0.0", "arm64", { draft: true }),
  ];
  assert.equal(
    selectRelease(releases, "0.0.1", "arm64", true).version,
    "0.0.2-alpha.10",
  );
  assert.equal(selectRelease(releases, "0.0.1", "arm64", false), null);
  assert.equal(selectRelease([release()], "0.0.2", "arm64", true), null);
  assert.equal(
    selectRelease([release("v0.0.2", "universal")], "0.0.1", "x64", true)
      .version,
    "0.0.2",
  );
});
test("unsafe assets and missing digests are rejected", () => {
  const bad = release();
  bad.assets[0].browser_download_url = "https://attacker.invalid/build.dmg";
  assert.throws(
    () => selectRelease([bad], "0.0.1", "arm64", true),
    /asset URL/,
  );
  bad.assets[0] = { ...release().assets[0], digest: null };
  assert.throws(() => selectRelease([bad], "0.0.1", "arm64", true), /SHA-256/);
});
test("checks, downloads once, verifies the DMG, deduplicates checks and rechecks before opening", async (t) => {
  let downloads = 0,
    calls = 0;
  const { updater, dir, opened } = await fixture(t, async (url) => {
    if (url.includes("api.github.com")) {
      calls++;
      return Response.json([release()]);
    }
    downloads++;
    return new Response(bytes);
  });
  const [one, two] = await Promise.all([updater.check(), updater.check()]);
  assert.equal(one.phase, "ready");
  assert.equal(two.phase, "ready");
  assert.equal(calls, 1);
  await updater.check();
  assert.equal(downloads, 1);
  await updater.openInstaller();
  assert.equal(opened.length, 1);
  await fs.writeFile(
    path.join(dir, "updates", release().assets[0].name),
    "tampered",
  );
  await assert.rejects(updater.openInstaller(), /changed/);
  assert.equal(opened.length, 1);
  await updater.download();
  assert.equal(updater.snapshot().phase, "ready");
});
test("corrupt or truncated downloads never become installable and partial files are removed", async (t) => {
  const { updater, dir, opened } = await fixture(t, async (url) =>
    url.includes("api.github.com")
      ? Response.json([release()])
      : new Response("bad"),
  );
  await updater.check();
  assert.equal(updater.snapshot().phase, "error");
  assert.match(updater.snapshot().error, /checksum/);
  assert.deepEqual(await fs.readdir(path.join(dir, "updates")), []);
  await assert.rejects(updater.openInstaller(), /verify/);
  assert.equal(opened.length, 0);
});
test("automatic download preferences persist and manual download still works", async (t) => {
  let downloads = 0;
  const { updater, dir } = await fixture(t, async (url) => {
    if (url.includes("api.github.com")) return Response.json([release()]);
    downloads++;
    return new Response(bytes);
  });
  await updater.configure({ autoDownload: false, autoCheck: false });
  await updater.check();
  assert.equal(updater.snapshot().phase, "available");
  assert.equal(downloads, 0);
  assert.equal(
    JSON.parse(await fs.readFile(path.join(dir, "updates.json"))).autoCheck,
    false,
  );
  await updater.download();
  assert.equal(downloads, 1);
  assert.equal(updater.snapshot().phase, "ready");
});
test("offline checks retain a verified installer and expose an actionable error", async (t) => {
  let offline = false;
  const { updater } = await fixture(t, async (url) => {
    if (offline) throw new Error("Network unavailable");
    return url.includes("api.github.com")
      ? Response.json([release()])
      : new Response(bytes);
  });
  await updater.check();
  offline = true;
  await updater.check();
  assert.equal(updater.snapshot().phase, "ready");
  assert.equal(updater.snapshot().release.version, "0.0.2");
  assert.match(updater.snapshot().error, /Network/);
});
test("redirects cannot send an update request outside GitHub's asset hosts", async (t) => {
  let external = false;
  const { updater } = await fixture(t, async (url) => {
    if (url.includes("api.github.com")) return Response.json([release()]);
    if (url.startsWith("https://github.com"))
      return new Response(null, {
        status: 302,
        headers: { Location: "https://attacker.invalid/package" },
      });
    external = true;
    return new Response(bytes);
  });
  await updater.check();
  assert.equal(external, false);
  assert.match(updater.snapshot().error, /Untrusted/);
});
test("release discovery follows pagination and handles GitHub rate limits", async (t) => {
  let pages = 0;
  const { updater } = await fixture(t, async (url) => {
    pages++;
    if (new URL(url).searchParams.get("page") === "1")
      return Response.json(
        Array.from({ length: 100 }, () => release("v0.0.1")),
      );
    return Response.json([release()]);
  });
  await updater.configure({ autoDownload: false });
  await updater.check();
  assert.equal(pages, 2);
  assert.equal(updater.snapshot().release.version, "0.0.2");
  updater.fetcher = async () => new Response(null, { status: 403 });
  await updater.check();
  assert.match(updater.snapshot().error, /rate limit/);
});

test("scheduled checks honor preferences and shutdown cancels in-flight requests", async (t) => {
  let calls = 0;
  const { updater } = await fixture(t, async () => {
    calls++;
    return Response.json([]);
  });
  updater.automatic = true;
  await updater.configure({ autoCheck: false });
  updater.schedule(5);
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(calls, 0);
  await updater.configure({ autoCheck: true });
  updater.schedule(5);
  for (let i = 0; i < 50 && !calls; i++)
    await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(calls, 1);
  assert.equal(updater.snapshot().phase, "current");
  updater.fetcher = (_, { signal }) =>
    new Promise((_, reject) =>
      signal.addEventListener(
        "abort",
        () => reject(new DOMException("Cancelled", "AbortError")),
        { once: true },
      ),
    );
  const check = updater.check();
  await updater.close();
  await check;
  assert.match(updater.snapshot().error, /interrupted/);
});

test("verified installers survive an offline restart and preference changes", async (t) => {
  const { updater, dir } = await fixture(t, async (url) =>
    url.includes("api.github.com")
      ? Response.json([release()])
      : new Response(bytes),
  );
  await updater.check();
  await updater.configure({ autoCheck: false });
  assert.equal(updater.snapshot().phase, "ready");
  const next = new Updates({
    directory: dir,
    currentVersion: "0.0.1-alpha.1",
    arch: "arm64",
    automatic: false,
    fetcher: async () => {
      throw Error("Offline");
    },
  });
  await next.init();
  assert.equal(next.snapshot().phase, "ready");
  assert.equal(next.snapshot().settings.autoCheck, false);
  await next.check();
  assert.equal(next.snapshot().phase, "ready");
  await next.close();
});

test("installation requires a verified download and an explicit request", async (t) => {
  const { updater } = await fixture(t, async (url) =>
    url.includes("api.github.com")
      ? Response.json([release()])
      : new Response(bytes),
  );
  let installs = 0;
  updater.installer = async () => {
    installs++;
  };
  updater.state.canInstall = true;
  await assert.rejects(updater.install(), /verify/);
  await updater.check();
  assert.equal(installs, 0);
  await updater.install();
  assert.equal(installs, 1);
  assert.equal(updater.snapshot().phase, "installing");
});
