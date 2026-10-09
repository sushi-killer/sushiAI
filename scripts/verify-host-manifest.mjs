#!/usr/bin/env node
// Fails when Resources/host/manifest.json of a packaged app no longer describes the bytes that
// ship (electron-builder re-signing rewrites Mach-O files; see "signIgnore" in package.json).
// Usage: node scripts/verify-host-manifest.mjs <path/to/sushiAI.app | path/to/host-dir>
import { createHash } from "node:crypto";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Returns { ok, results: [{ key, path, ok, reason? }] } for every manifest entry.
export function verifyHostManifest(appOrHostDir) {
  const hostDir = existsSync(join(appOrHostDir, "Contents"))
    ? join(appOrHostDir, "Contents", "Resources", "host")
    : appOrHostDir;
  const manifestPath = join(hostDir, "manifest.json");
  if (!existsSync(manifestPath)) {
    return {
      ok: false,
      results: [
        { key: "manifest", path: manifestPath, ok: false, reason: "missing" },
      ],
    };
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const results = Object.entries(manifest).map(([key, entry]) => {
    const file = join(hostDir, entry.path);
    if (!existsSync(file))
      return { key, path: entry.path, ok: false, reason: "file missing" };
    const bytes = readFileSync(file);
    const digest = createHash("sha256").update(bytes).digest("hex");
    if (digest !== entry.sha256)
      return { key, path: entry.path, ok: false, reason: "sha256 differs" };
    if (statSync(file).size !== entry.size)
      return { key, path: entry.path, ok: false, reason: "size differs" };
    return { key, path: entry.path, ok: true };
  });
  if (results.length === 0)
    results.push({
      key: "manifest",
      path: manifestPath,
      ok: false,
      reason: "empty",
    });
  return { ok: results.every((r) => r.ok), results };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const target = process.argv[2];
  if (!target) {
    console.error("Usage: verify-host-manifest.mjs <sushiAI.app>");
    process.exit(2);
  }
  const { ok, results } = verifyHostManifest(resolve(target));
  for (const r of results)
    console.log(
      `${r.ok ? "OK  " : "FAIL"} ${r.path}${r.reason ? ` (${r.reason})` : ""}`,
    );
  process.exit(ok ? 0 : 1);
}
