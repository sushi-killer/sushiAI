const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { createReadStream } = require("node:fs");

const REPOSITORY = "sushi-killer/sushiAI";
const RELEASES_URL = `https://github.com/${REPOSITORY}/releases`;
const INTERVAL = 6 * 60 * 60 * 1000;
const MAX_DOWNLOAD = 1024 * 1024 * 1024;

function version(value) {
  const match =
    /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\da-zA-Z-]+(?:\.[\da-zA-Z-]+)*))?(?:\+[\da-zA-Z-]+(?:\.[\da-zA-Z-]+)*)?$/.exec(
      value || "",
    );
  if (!match) return null;
  const parts = match.slice(1, 4).map(Number);
  const pre = match[4]?.split(".") || [];
  if (
    parts.some((n) => !Number.isSafeInteger(n)) ||
    pre.some((s) => /^\d+$/.test(s) && s.length > 1 && s[0] === "0")
  )
    return null;
  return { parts, pre };
}
function compareVersions(a, b) {
  const x = version(a),
    y = version(b);
  if (!x || !y) throw new Error("Invalid release version.");
  for (let i = 0; i < 3; i++)
    if (x.parts[i] !== y.parts[i]) return Math.sign(x.parts[i] - y.parts[i]);
  if (!x.pre.length || !y.pre.length)
    return x.pre.length === y.pre.length ? 0 : x.pre.length ? -1 : 1;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const l = x.pre[i],
      r = y.pre[i];
    if (l === r) continue;
    if (l === undefined || r === undefined) return l === undefined ? -1 : 1;
    const ln = /^\d+$/.test(l),
      rn = /^\d+$/.test(r);
    if (ln && rn) return BigInt(l) < BigInt(r) ? -1 : 1;
    if (ln !== rn) return ln ? -1 : 1;
    return l < r ? -1 : 1;
  }
  return 0;
}
function assetURL(value) {
  const url = new URL(value);
  if (
    url.origin !== "https://github.com" ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    !url.pathname.startsWith(`/${REPOSITORY}/releases/download/`)
  )
    throw new Error("Invalid release asset URL.");
  return url.href;
}
function selectRelease(releases, current, arch, includePrereleases) {
  const newer = releases.filter(
    (r) =>
      !r.draft &&
      version(r.tag_name) &&
      (includePrereleases ||
        (!r.prerelease && !version(r.tag_name).pre.length)) &&
      compareVersions(r.tag_name, current) > 0,
  );
  newer.sort((a, b) => compareVersions(b.tag_name, a.tag_name));
  for (const release of newer) {
    const v = release.tag_name.replace(/^v/, "");
    const names = [`sushiAI-${v}-${arch}.dmg`, `sushiAI-${v}-universal.dmg`];
    const asset = names
      .map((name) =>
        release.assets?.find((a) => a.name === name && a.state === "uploaded"),
      )
      .find(Boolean);
    if (!asset) continue;
    // Fail closed when a published build has no trustworthy checksum.
    if (!/^sha256:[a-f0-9]{64}$/i.test(asset.digest || ""))
      throw new Error(
        `Release ${v} has no SHA-256 digest. Contact the maintainer.`,
      );
    if (
      !Number.isSafeInteger(asset.size) ||
      asset.size < 1 ||
      asset.size > MAX_DOWNLOAD
    )
      throw new Error("Invalid update package size.");
    return {
      version: v,
      name: asset.name,
      url: assetURL(asset.browser_download_url),
      sha256: asset.digest.slice(7).toLowerCase(),
      size: asset.size,
      notes: String(release.body || "").slice(0, 12000),
    };
  }
  return null;
}
async function hashFile(file) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest("hex");
}

class Updates {
  constructor({
    directory,
    currentVersion,
    arch = process.arch,
    fetcher = fetch,
    openPath,
    openExternal,
    onChange = () => {},
    automatic = true,
    installer,
    canInstall = false,
  }) {
    this.directory = path.join(directory, "updates");
    this.settingsFile = path.join(directory, "updates.json");
    this.fetcher = fetcher;
    this.openPath = openPath;
    this.openExternal = openExternal;
    this.onChange = onChange;
    this.automatic = automatic;
    this.arch = arch;
    this.installer = installer;
    this.state = {
      currentVersion,
      canInstall,
      repository: REPOSITORY,
      settings: {
        autoCheck: true,
        autoDownload: true,
        includePrereleases: !!version(currentVersion)?.pre.length,
      },
      phase: "idle",
      release: null,
      progress: 0,
      checkedAt: null,
      error: null,
    };
  }
  snapshot() {
    return structuredClone(this.state);
  }
  emit(patch) {
    Object.assign(this.state, patch);
    this.onChange(this.snapshot());
  }
  async init() {
    await fs.mkdir(this.directory, { recursive: true, mode: 0o700 });
    try {
      const saved = JSON.parse(await fs.readFile(this.settingsFile, "utf8"));
      for (const key of Object.keys(this.state.settings))
        if (typeof saved[key] === "boolean")
          this.state.settings[key] = saved[key];
    } catch (e) {
      if (e.code !== "ENOENT")
        this.emit({
          error: "Update preferences could not be loaded. Defaults are in use.",
        });
    }
    try {
      const message = await fs.readFile(
        path.join(this.directory, "install-error.txt"),
        "utf8",
      );
      this.emit({ error: message.slice(0, 4000) });
      await fs.rm(path.join(this.directory, "install-error.txt"), {
        force: true,
      });
    } catch {}
    // Remove only abandoned partial downloads owned by this updater.
    for (const name of await fs.readdir(this.directory))
      if (/^sushiAI-[\w.+-]+\.dmg\.part$/.test(name))
        await fs.rm(path.join(this.directory, name), { force: true });
    try {
      const receipt = JSON.parse(
        await fs.readFile(path.join(this.directory, "ready.json"), "utf8"),
      );
      const r = receipt.release;
      const release = selectRelease(
        [
          {
            tag_name: r.version,
            assets: [
              {
                name: r.name,
                state: "uploaded",
                size: r.size,
                digest: `sha256:${r.sha256}`,
                browser_download_url: r.url,
              },
            ],
            body: r.notes,
          },
        ],
        this.state.currentVersion,
        this.arch,
        this.state.settings.includePrereleases,
      );
      if (release) {
        const file = path.join(this.directory, release.name);
        if (
          (await fs.stat(file)).size === release.size &&
          (await hashFile(file)) === release.sha256
        )
          this.emit({
            release,
            phase: "ready",
            progress: 100,
            checkedAt:
              typeof receipt.checkedAt === "string" &&
              !Number.isNaN(Date.parse(receipt.checkedAt))
                ? receipt.checkedAt
                : null,
          });
      }
    } catch {
      /* A missing or invalid receipt is recovered by the next check. */
    }
    this.schedule(15000);
    return this.snapshot();
  }
  schedule(delay = INTERVAL) {
    clearTimeout(this.timer);
    if (!this.closed && this.automatic && this.state.settings.autoCheck) {
      this.timer = setTimeout(
        () => this.check().finally(() => this.schedule()),
        delay,
      );
      this.timer.unref?.();
    }
  }
  async configure(patch) {
    if (this.job)
      throw new Error("Wait for the current update operation to finish.");
    const settings = { ...this.state.settings };
    for (const [key, value] of Object.entries(patch || {})) {
      if (!Object.hasOwn(settings, key) || typeof value !== "boolean")
        throw new Error("Invalid update preference.");
      settings[key] = value;
    }
    await fs.writeFile(this.settingsFile + ".tmp", JSON.stringify(settings), {
      mode: 0o600,
    });
    await fs.rename(this.settingsFile + ".tmp", this.settingsFile);
    const excluded =
      this.state.release &&
      !settings.includePrereleases &&
      version(this.state.release.version).pre.length;
    this.emit({
      settings,
      ...(excluded ? { phase: "idle", release: null, progress: 0 } : {}),
      error: null,
    });
    this.schedule(1000);
    return this.snapshot();
  }
  async response(url, signal) {
    // GitHub redirects assets to its signed CDN URLs. Never follow arbitrary hosts.
    for (let redirects = 0; redirects <= 5; redirects++) {
      const u = new URL(url);
      if (
        u.protocol !== "https:" ||
        u.username ||
        u.password ||
        ![
          "api.github.com",
          "github.com",
          "release-assets.githubusercontent.com",
          "objects.githubusercontent.com",
        ].includes(u.hostname)
      )
        throw new Error("Untrusted update download host.");
      const response = await this.fetcher(u.href, {
        signal,
        redirect: "manual",
        headers: {
          "User-Agent": "sushiAI-updater",
          ...(u.hostname === "api.github.com"
            ? {
                Accept: "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
              }
            : {}),
        },
      });
      if ([301, 302, 303, 307, 308].includes(response.status)) {
        const location = response.headers.get("location");
        await response.body?.cancel();
        if (!location) throw new Error("Missing update redirect.");
        url = new URL(location, u).href;
        continue;
      }
      if (!response.ok) {
        await response.body?.cancel();
        throw new Error(
          response.status === 403 || response.status === 429
            ? "GitHub rate limit reached. Try again later."
            : `GitHub returned HTTP ${response.status}. Try again later.`,
        );
      }
      return response;
    }
    throw new Error("Too many update redirects.");
  }
  async releases(signal) {
    signal = AbortSignal.any([signal, AbortSignal.timeout(30000)]);
    const releases = [];
    for (let page = 1; page <= 5; page++) {
      const response = await this.response(
        `https://api.github.com/repos/${REPOSITORY}/releases?per_page=100&page=${page}`,
        signal,
      );
      const chunks = [];
      let bytes = 0;
      for await (const chunk of response.body) {
        bytes += chunk.length;
        if (bytes > 8 * 1024 * 1024)
          throw new Error("Release metadata is too large.");
        chunks.push(chunk);
      }
      const items = JSON.parse(Buffer.concat(chunks).toString());
      if (!Array.isArray(items))
        throw new Error("Invalid GitHub release response.");
      releases.push(...items);
      if (items.length < 100) return releases;
    }
    throw new Error("Too many releases to check reliably.");
  }
  check() {
    return this.run(
      async (signal) => {
        const previous = this.state.release;
        const previousPhase = this.state.phase;
        this.emit({ phase: "checking", error: null });
        try {
          const release = selectRelease(
            await this.releases(signal),
            this.state.currentVersion,
            this.arch,
            this.state.settings.includePrereleases,
          );
          this.emit({
            release,
            checkedAt: new Date().toISOString(),
            progress: 0,
            phase: release ? "available" : "current",
          });
          if (release && this.state.settings.autoDownload)
            await this.downloadFile(release, signal);
        } catch (error) {
          // A transient check failure must not discard a verified downloaded update.
          if (this.state.phase === "checking")
            this.emit({
              release: previous,
              phase:
                previousPhase === "ready"
                  ? "ready"
                  : previous
                    ? "available"
                    : "error",
            });
          throw error;
        }
      },
      15 * 60 * 1000,
    );
  }
  download() {
    return this.run(
      async (signal) => {
        if (!this.state.release) throw new Error("Check for an update first.");
        await this.downloadFile(this.state.release, signal);
      },
      15 * 60 * 1000,
    );
  }
  run(action, timeout) {
    if (this.job) return this.job.then(() => this.snapshot());
    this.controller = new AbortController();
    const signal = AbortSignal.any([
      this.controller.signal,
      AbortSignal.timeout(timeout),
    ]);
    this.job = action(signal)
      .catch((error) => {
        this.emit({
          phase: this.state.phase === "ready" ? "ready" : "error",
          error:
            error.name === "AbortError" || error.name === "TimeoutError"
              ? "Update request interrupted. Try again."
              : error.message,
        });
      })
      .finally(() => {
        this.job = null;
        this.controller = null;
      });
    return this.job.then(() => this.snapshot());
  }
  async markReady(release) {
    const receipt = path.join(this.directory, "ready.json");
    await fs.writeFile(
      receipt + ".tmp",
      JSON.stringify({ release, checkedAt: this.state.checkedAt }),
      { mode: 0o600 },
    );
    await fs.rename(receipt + ".tmp", receipt);
    this.emit({ phase: "ready", progress: 100 });
  }
  async downloadFile(release, signal) {
    const destination = path.join(this.directory, release.name);
    this.emit({ phase: "downloading", progress: 0, error: null });
    try {
      if (
        (await fs.stat(destination)).size === release.size &&
        (await hashFile(destination)) === release.sha256
      ) {
        await this.markReady(release);
        return;
      }
    } catch (e) {
      if (e.code !== "ENOENT") throw e;
    }
    const partial = destination + ".part";
    let file;
    try {
      const response = await this.response(release.url, signal);
      const hash = createHash("sha256");
      let size = 0,
        last = 0;
      file = await fs.open(partial, "w", 0o600);
      for await (const chunk of response.body) {
        size += chunk.length;
        if (size > release.size)
          throw new Error("Update package exceeds its expected size.");
        hash.update(chunk);
        await file.writeFile(chunk);
        if (Date.now() - last > 200) {
          this.emit({ progress: Math.floor((size / release.size) * 100) });
          last = Date.now();
        }
      }
      if (size !== release.size || hash.digest("hex") !== release.sha256)
        throw new Error(
          "Update checksum verification failed. Download the package again.",
        );
      await file.sync();
      await file.close();
      file = null;
      await fs.rename(partial, destination);
      await this.markReady(release);
      for (const name of await fs.readdir(this.directory))
        if (name !== release.name && /^sushiAI-[\w.+-]+\.dmg$/.test(name))
          await fs
            .rm(path.join(this.directory, name), { force: true })
            .catch(() => {});
    } finally {
      await file?.close();
      await fs.rm(partial, { force: true });
    }
  }
  async install() {
    if (this.job || this.state.phase !== "ready" || !this.state.release)
      throw new Error("Download and verify the update first.");
    if (!this.state.canInstall || !this.installer)
      throw new Error(
        "Automatic installation requires the packaged macOS app.",
      );
    return this.run(
      async () => {
        const release = this.state.release;
        const file = path.join(this.directory, release.name);
        this.emit({ phase: "installing", error: null });
        if ((await hashFile(file)) !== release.sha256)
          throw new Error("Downloaded package changed. Download it again.");
        await this.installer(release, file);
      },
      15 * 60 * 1000,
    );
  }
  async openInstaller() {
    if (this.job || this.state.phase !== "ready" || !this.state.release)
      throw new Error("Download and verify the update first.");
    const file = path.join(this.directory, this.state.release.name);
    if ((await hashFile(file)) !== this.state.release.sha256) {
      this.emit({
        phase: "error",
        error: "Downloaded package changed. Download it again.",
      });
      throw new Error(this.state.error);
    }
    const error = await this.openPath(file);
    if (error) throw new Error(error);
  }
  releasePage() {
    return this.openExternal(RELEASES_URL);
  }
  close() {
    this.closed = true;
    clearTimeout(this.timer);
    this.controller?.abort();
    return this.job;
  }
}
module.exports = { Updates, compareVersions, selectRelease, assetURL };
