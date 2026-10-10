"use strict";

/**
 * Read-only HashiCorp Vault KV v2 as a source of a project's values on This
 * Mac. The token is kept encrypted (safeStorage) in the app database; values
 * live in memory only, are never written or logged, and reach a project
 * through `projects.vault`, so the project store never loads this module.
 */
const { readStore, writeStore } = require("./app-db.cjs");

const STORE = "vault-token";
const CACHE_TTL_MS = 5 * 60 * 1000;
const TIMEOUT_MS = 5000;
const MAX_TOKEN = 4096;
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

const NO_TOKEN = "Vault: no token saved";
const UNREACHABLE = "Vault: unreachable";
const TIMED_OUT = "Vault: timed out";
const UNREADABLE = "Vault: unreadable reply";

/** What a failed status means to the owner; never the reply itself. */
function statusReason(status) {
  if (status === 401 || status === 403) return "Vault: no access (403)";
  if (status === 404) return "Vault: path not found (404)";
  return `Vault: error (${status})`;
}

const timedOut = (error) =>
  error?.name === "TimeoutError" || error?.cause?.name === "TimeoutError";

/** `a/b c` -> `a/b%20c`: each segment encoded, the slashes kept. */
const segments = (value) => value.split("/").map(encodeURIComponent).join("/");

/** The values of one KV v2 path, or the reason it could not be read. A
 * redirect is not followed: the token would go to wherever it points. */
async function readPath(token, { address, mount }, secretPath) {
  let response;
  try {
    response = await fetch(
      `${address}/v1/${segments(mount)}/data/${segments(secretPath)}`,
      {
        headers: { "X-Vault-Token": token },
        redirect: "manual",
        signal: AbortSignal.timeout(TIMEOUT_MS),
      },
    );
  } catch (error) {
    return { ok: false, reason: timedOut(error) ? TIMED_OUT : UNREACHABLE };
  }
  if (!response.ok) {
    response.body?.cancel().catch(() => {});
    return { ok: false, reason: statusReason(response.status) };
  }
  let body;
  try {
    body = await response.json();
  } catch (error) {
    return { ok: false, reason: timedOut(error) ? TIMED_OUT : UNREADABLE };
  }
  const data = body?.data?.data;
  if (!data || typeof data !== "object" || Array.isArray(data))
    return { ok: false, reason: UNREADABLE };
  const values = {};
  for (const [name, value] of Object.entries(data))
    if (
      NAME.test(name) &&
      ["string", "number", "boolean"].includes(typeof value)
    )
      values[name] = String(value);
  return { ok: true, values };
}

/** Every path of a project's config merged in order (a later path wins), or
 * the first failure: one failed path drops every value. */
async function readAll(token, config) {
  if (!token) return { ok: false, reason: NO_TOKEN };
  const values = {};
  for (const secretPath of config.paths) {
    const result = await readPath(token, config, secretPath).catch(() => ({
      ok: false,
      reason: UNREACHABLE,
    }));
    if (!result.ok) return result;
    Object.assign(values, result.values);
  }
  return { ok: true, values };
}

function registerVault({
  handle,
  projects,
  notices,
  safeStorage,
  userDataDir,
}) {
  // address+mount+paths -> { at, result: Promise, notified: Set<projectId> }
  const cache = new Map();
  // Projects with a failure notice on screen, retracted by a good read.
  const noticed = new Set();
  // Bumped by a token change, so a read started before it is not cached.
  let generation = 0;

  const cacheKey = ({ address, mount, paths }) =>
    JSON.stringify([address, mount, paths]);

  function readToken() {
    const stored = readStore(userDataDir, STORE).token;
    if (typeof stored?.ct !== "string") return null;
    try {
      return (
        safeStorage.decryptString(Buffer.from(stored.ct, "base64")) || null
      );
    } catch {
      return null;
    }
  }

  function setToken(token) {
    if (typeof token !== "string") throw new Error("Enter a Vault token.");
    const value = token.trim();
    if (!value || value.length > MAX_TOKEN || !/^[\x21-\x7e]+$/.test(value))
      throw new Error("Enter a Vault token.");
    if (
      !safeStorage?.isEncryptionAvailable?.() ||
      safeStorage.getSelectedStorageBackend?.() === "basic_text"
    )
      throw new Error("Secure storage is unavailable.");
    writeStore(userDataDir, STORE, {
      token: {
        v: 1,
        ct: safeStorage.encryptString(value).toString("base64"),
      },
    });
    generation += 1;
    cache.clear();
    return { saved: true };
  }

  function clearToken() {
    writeStore(userDataDir, STORE, {});
    generation += 1;
    cache.clear();
    return { saved: false };
  }

  function entryFor(config) {
    const key = cacheKey(config);
    const cached = cache.get(key);
    if (cached && Date.now() - cached.at < CACHE_TTL_MS) return cached;
    const entry = {
      at: Date.now(),
      result: readAll(readToken(), config),
      notified: new Set(),
    };
    cache.set(key, entry);
    return entry;
  }

  function report(project, entry, result) {
    const key = `vault:${project.id}`;
    try {
      if (result.ok) {
        if (noticed.delete(project.id)) notices.retract("vault", key);
        return;
      }
      if (entry.notified.has(project.id)) return;
      entry.notified.add(project.id);
      noticed.add(project.id);
      notices.publish("vault", {
        key,
        kind: "failed",
        title: "Vault",
        header: project.name,
        body: result.reason,
      });
    } catch {
      // A notice that cannot be shown never stops a project's values.
    }
  }

  projects.vault = {
    /** The project's Vault values, {} when they cannot all be read. */
    async values(project) {
      const entry = entryFor(project.vault);
      const result = await entry.result;
      report(project, entry, result);
      return result.ok ? { ...result.values } : {};
    },
  };

  notices.register("vault", async () => {});

  handle("vault:token:status", () => ({ saved: Boolean(readToken()) }));
  handle("vault:token:set", (token) => setToken(token));
  handle("vault:token:clear", () => clearToken());
  handle("projects:vault:set", (id, config) => projects.setVault(id, config));
  // A fresh read for the owner's Check: it replaces the cached one and sends
  // no notice. Names and a count only, never a value.
  handle("projects:vault:check", async (id) => {
    const project = await projects.get(id);
    if (!project) throw new Error("Unknown project.");
    if (!project.vault?.paths?.length)
      throw new Error("The project has no Vault paths.");
    const started = generation;
    const result = await readAll(readToken(), project.vault);
    if (started === generation)
      cache.set(cacheKey(project.vault), {
        at: Date.now(),
        result: Promise.resolve(result),
        notified: new Set(),
      });
    if (!result.ok) return { ok: false, reason: result.reason };
    if (noticed.delete(project.id))
      notices.retract("vault", `vault:${project.id}`);
    const names = Object.keys(result.values).sort();
    return { ok: true, names, count: names.length };
  });
}

module.exports = { registerVault, CACHE_TTL_MS, STORE };
