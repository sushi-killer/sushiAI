// The data behind the macOS rule of scripts/ci-changes.mjs, read from the
// workspace itself (no list of crates or tests lives in scripts/):
// - platform crates: workspace crates whose own Cargo.toml names a
//   platform-only dependency (a `[target.'cfg(...)'.dependencies]` table or a
//   crate from PLATFORM_DEPS);
// - macOS-only files: Rust files that carry a `// ci-platform: macos-only`
//   line next to a test that failed on the macOS runner while ubuntu passed it
//   (the line names the CI run that shows it).
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

// Crates whose job is to bind one operating system's API: a change next to
// them is a change to platform behaviour.
const PLATFORM_DEPS = [
  "portable-pty",
  "nix",
  "rustix",
  "signal-hook",
  "windows-sys",
  "core-foundation",
  "security-framework",
];
export const MACOS_ONLY = /\/\/ ci-platform: macos-only\b/;

function rustFiles(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "target" || entry.name.startsWith(".")) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) rustFiles(full, out);
    else if (entry.name.endsWith(".rs")) out.push(full);
  }
  return out;
}

export function platformCrates(base = root) {
  const crates = path.join(base, "crates");
  return fs
    .readdirSync(crates, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => `crates/${e.name}`)
    .filter((dir) => {
      const manifest = path.join(base, dir, "Cargo.toml");
      if (!fs.existsSync(manifest)) return false;
      const text = fs.readFileSync(manifest, "utf8");
      if (/^\[target\..*dependencies\]/m.test(text)) return true;
      const deps = text
        .split(/^\[/m)
        .filter((section) => /^[\w.-]*dependencies\]/.test(section))
        .flatMap((section) => [...section.matchAll(/^([\w-]+)\s*[.=]/gm)])
        .map((m) => m[1]);
      return deps.some((d) => PLATFORM_DEPS.includes(d));
    })
    .sort();
}

/** The Rust files under crates/ marked `// ci-platform: macos-only`. */
export function macosOnlyFiles(base = root) {
  return rustFiles(path.join(base, "crates"))
    .filter((file) => MACOS_ONLY.test(fs.readFileSync(file, "utf8")))
    .map((file) => path.relative(base, file).split(path.sep).join("/"))
    .sort();
}

let cache = null;
export function readData() {
  cache ??= { crates: platformCrates(), macosOnly: macosOnlyFiles() };
  return cache;
}
