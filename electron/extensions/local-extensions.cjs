const path = require("node:path");
const { mkdir, readdir, readFile, realpath, stat } = require("node:fs/promises");
const { validateExtensionManifest } = require("./manifest.cjs");

// A manifest is a single JSON document that travels to the renderer over IPC,
// so it is read whole into memory. The bundled fixture is about 4 KiB.
const MAX_MANIFEST_BYTES = 256 * 1024;

/** Scans one level of an extensions folder. Every folder is read on its own, so
 * one broken manifest costs only its own entry: the rest still load and the
 * failure is reported back as a problem the user can act on.
 *
 * Symlinked folders are followed on purpose - pointing the extensions folder at
 * a checkout elsewhere is how you develop an extension, the same way `npm link`
 * works. Nothing from the folder is ever executed; only this JSON is read. */
async function scanLocalExtensions(root, reservedIds = []) {
  const manifests = new Map();
  const problems = [];
  let entries;
  try {
    await mkdir(root, { recursive: true });
    entries = await readdir(root, { withFileTypes: true });
  } catch (error) {
    return {
      manifests,
      problems: [
        {
          folder: path.basename(root),
          path: root,
          error: `Extensions folder is unreadable: ${error?.message || error}`,
        },
      ],
    };
  }

  const taken = new Map(reservedIds.map((id) => [id, null]));
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    if (entry.name.startsWith(".")) continue;
    const folder = path.join(root, entry.name);
    let directory = entry.isDirectory();
    if (entry.isSymbolicLink())
      directory = await stat(folder)
        .then((info) => info.isDirectory())
        .catch(() => false);
    if (!directory) continue;

    const problem = (error) =>
      problems.push({ folder: entry.name, path: folder, error });
    try {
      const resolved = await realpath(folder);
      const file = path.join(resolved, "manifest.json");
      const info = await stat(file).catch(() => null);
      if (!info || !info.isFile()) {
        problem("manifest.json not found");
        continue;
      }
      if (info.size > MAX_MANIFEST_BYTES) {
        problem(
          `manifest.json is larger than ${MAX_MANIFEST_BYTES / 1024} KiB`,
        );
        continue;
      }
      let raw;
      try {
        raw = JSON.parse(await readFile(file, "utf8"));
      } catch (error) {
        problem(`manifest.json: ${error?.message || error}`);
        continue;
      }
      if (
        raw &&
        typeof raw === "object" &&
        raw.source !== undefined &&
        !(
          raw.source &&
          typeof raw.source === "object" &&
          raw.source.kind === "local"
        )
      ) {
        problem("A local extension must not declare a source.");
        continue;
      }
      const manifest = validateExtensionManifest({
        ...raw,
        source: { kind: "local", path: resolved },
      });
      if (taken.has(manifest.id)) {
        const other = taken.get(manifest.id);
        problem(`Extension id ${manifest.id} is already in use`);
        // The folder that claimed the id first loses it too: leaving one of two
        // identical ids loaded would depend on directory order.
        if (other) {
          manifests.delete(manifest.id);
          problems.push({
            folder: other.folder,
            path: other.path,
            error: `Extension id ${manifest.id} is already in use`,
          });
        }
        continue;
      }
      taken.set(manifest.id, { folder: entry.name, path: folder });
      manifests.set(manifest.id, manifest);
    } catch (error) {
      problem(String(error?.message || error));
    }
  }
  return { manifests, problems };
}

module.exports = { scanLocalExtensions, MAX_MANIFEST_BYTES };
