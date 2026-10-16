const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const library = require("../electron/extensions/manifest.cjs");
const { VAULT_MANIFEST } = require("../electron/extensions/builtin-vault.cjs");
const {
  ROUTINES_MANIFEST,
} = require("../electron/extensions/builtin-routines.cjs");
const {
  ORCHESTRATOR_MANIFEST,
} = require("../electron/extensions/builtin-orchestrator.cjs");
const {
  ARTIFACTS_MANIFEST,
} = require("../electron/extensions/builtin-artifacts.cjs");

const { validateExtensionManifest, CONTRACT } = library;

const fixture = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "fixtures/extensions/probe/manifest.json"),
    "utf8",
  ),
);

const withTop = (icon, mutate) => {
  const base = structuredClone(fixture);
  base.source = { kind: "npm", package: "@example/probe", version: "1.2.3" };
  if (icon !== undefined) base.icon = icon;
  if (mutate) mutate(base);
  return base;
};

test("an extension-level icon is inherited by surfaces and navigation without their own", () => {
  const base = withTop("folder", (m) => {
    delete m.contributions.surfaces[0].icon;
  });
  const result = validateExtensionManifest(base);
  assert.deepEqual(result.contributions.surfaces[0].icon, {
    kind: "named",
    name: "folder",
  });
});

test("an extension-level icon accepts an svg object and rejects unknown names", () => {
  const svg = {
    kind: "svg",
    viewBox: "0 0 24 24",
    paths: ["M4 4h16v16H4z"],
  };
  const ok = validateExtensionManifest(
    withTop(svg, (m) => {
      delete m.contributions.surfaces[0].icon;
    }),
  );
  assert.equal(ok.contributions.surfaces[0].icon.kind, "svg");
  assert.throws(
    () => validateExtensionManifest(withTop("skull")),
    /is not a bundled icon/,
  );
});

test("a surface's own icon wins over the extension icon", () => {
  const base = withTop("folder", (m) => {
    m.contributions.surfaces[0].icon = "sparkles";
  });
  const result = validateExtensionManifest(base);
  assert.equal(result.contributions.surfaces[0].icon.name, "sparkles");
});

test("the extension icon is additive and does not bump the API version", () => {
  assert.deepEqual([...CONTRACT.SUPPORTED_API_VERSIONS], [1]);
});

test("built-in settings tabs resolve to distinct bundled non-fallback icons", () => {
  const seen = new Map();
  for (const raw of [
    VAULT_MANIFEST,
    ROUTINES_MANIFEST,
    ORCHESTRATOR_MANIFEST,
    ARTIFACTS_MANIFEST,
  ]) {
    const result = validateExtensionManifest(structuredClone(raw));
    for (const surface of result.contributions.surfaces) {
      assert.ok(
        surface.icon && surface.icon.kind === "named",
        `${result.id}:${surface.id} has no icon`,
      );
      assert.notEqual(
        surface.icon.name,
        "plug",
        `${result.id}:${surface.id} uses the fallback`,
      );
      if (surface.allowedHosts.includes("settings.page")) {
        const key = surface.icon.name;
        assert.ok(
          !seen.has(key),
          `${result.id}:${surface.id} repeats ${key} from ${seen.get(key)}`,
        );
        seen.set(key, `${result.id}:${surface.id}`);
      }
    }
  }
  assert.ok(seen.size >= 3);
});

test("the renderer draws every icon with one shared stroke and a fixed box", () => {
  const source = fs.readFileSync("src/extensions/ExtensionSlots.tsx", "utf8");
  assert.match(source, /ICON_STROKE/);
  assert.match(source, /extension-icon/);
});
