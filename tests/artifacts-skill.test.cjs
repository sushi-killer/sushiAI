const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");
const {
  ARTIFACTS_MANIFEST,
  ARTIFACTS_SKILL,
} = require("../electron/extensions/builtin-artifacts.cjs");
const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");
const {
  configureBuiltinSkillSync,
  registerExtensionSkill,
  syncBuiltinSkillsOnHost,
  syncLocalBuiltinSkills,
  installBuiltinSkillsInto,
} = require("../electron/extensions/builtin-skills.cjs");

const NAME = ARTIFACTS_SKILL.name;
const SKILL_TEXT = fs.readFileSync(
  path.join(__dirname, "../electron/extensions/artifacts-skill.md"),
  "utf8",
);
/** Registers the Artifacts skill the way the composition point does. */
function configureArtifactsSkill(options) {
  registerExtensionSkill({
    extensionId: ARTIFACTS_MANIFEST.id,
    ...ARTIFACTS_SKILL,
    ...options,
  });
}
configureArtifactsSkill({ isEnabled: () => true });

/** Points this machine's sync at a fake binary that records each run. */
function recordRuns({ accounts = null, env = {}, fail = false } = {}) {
  const runs = [];
  configureBuiltinSkillSync({
    binary: () => "/opt/sushiai/bin/sushiai",
    codexAccountsDir: accounts,
    env,
    run: async (binary, args, options) => {
      runs.push({
        binary,
        args,
        input: options.input,
        codexHome: options.env.CODEX_HOME,
      });
      if (fail) throw new Error("unreachable binary");
      return `written /x/${args[2]}/SKILL.md\n`;
    },
  });
  return runs;
}

function tempHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "artifacts-skill-"));
}

test("this machine's sync runs the binary once, then once per Codex account home", async () => {
  const home = tempHome();
  try {
    const accounts = path.join(home, "codex-accounts");
    fs.mkdirSync(path.join(accounts, "a1"), { recursive: true });
    fs.mkdirSync(path.join(accounts, "b2"), { recursive: true });
    const runs = recordRuns({ accounts, env: { CODEX_HOME: "/custom" } });
    const changed = await syncLocalBuiltinSkills();
    assert.deepEqual(
      runs.map((r) => [r.binary, r.args.join(" "), r.codexHome]),
      [
        ["/opt/sushiai/bin/sushiai", `skills install ${NAME}`, "/custom"],
        [
          "/opt/sushiai/bin/sushiai",
          `skills install ${NAME}`,
          path.join(accounts, "a1"),
        ],
        [
          "/opt/sushiai/bin/sushiai",
          `skills install ${NAME}`,
          path.join(accounts, "b2"),
        ],
      ],
    );
    assert.ok(runs.every((r) => r.input === SKILL_TEXT));
    assert.equal(changed.length, 3);
    // Nothing of this file writes a home itself.
    assert.deepEqual(fs.readdirSync(home), ["codex-accounts"]);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("a new Codex account home gets the enabled skills with CODEX_HOME set", async () => {
  const runs = recordRuns();
  await installBuiltinSkillsInto("/data/codex-accounts/new");
  assert.deepEqual(
    runs.map((r) => [r.args.join(" "), r.codexHome]),
    [[`skills install ${NAME}`, "/data/codex-accounts/new"]],
  );
});

test("an SSH host runs its own installed sushiai with the text on stdin", async () => {
  const calls = [];
  const exec = async (endpoint, command, { input }) => {
    calls.push({ endpoint, command, input });
    return "";
  };
  const ok = await syncBuiltinSkillsOnHost({ exec }, "ssh:user@devbox", {});
  assert.equal(ok, true);
  assert.deepEqual(calls, [
    {
      endpoint: "ssh:user@devbox",
      command: `"$HOME/.sushiai/bin/sushiai" skills install ${NAME}`,
      input: SKILL_TEXT,
    },
  ]);
});

test("a failing binary or host exec never throws", async () => {
  const warn = console.warn;
  console.warn = () => {};
  try {
    recordRuns({ fail: true });
    assert.deepEqual(await syncLocalBuiltinSkills(), []);
    const exec = async () => {
      throw new Error("unreachable 192.0.2.7");
    };
    assert.equal(
      await syncBuiltinSkillsOnHost({ exec }, "ssh:user@devbox", {}),
      false,
    );
    configureBuiltinSkillSync({
      binary: () => {
        throw new Error("sushiai binary not found");
      },
    });
    assert.deepEqual(await syncLocalBuiltinSkills(), []);
  } finally {
    console.warn = warn;
  }
});

test("a test launch never runs the binary", async () => {
  const runs = recordRuns({ env: { SUSHIAI_TEST_WINDOW: "hidden" } });
  assert.deepEqual(await syncLocalBuiltinSkills(), []);
  assert.equal(
    await syncBuiltinSkillsOnHost({ exec: assert.fail }, "ssh:u@h", {
      SUSHIAI_TEST_WINDOW: "hidden",
    }),
    false,
  );
  assert.deepEqual(runs, []);
});

test("turning the extension off removes the skill, on installs it again; another extension does nothing", async () => {
  let enabled = true;
  let listener;
  configureArtifactsSkill({
    isEnabled: () => enabled,
    subscribe: (fn) => (listener = fn),
  });
  try {
    const runs = recordRuns();
    enabled = false;
    await listener("builtin.artifacts", false);
    assert.equal(listener("builtin.other", true), undefined);
    enabled = true;
    await listener("builtin.artifacts", true);
    assert.deepEqual(
      runs.map((r) => [r.args.join(" "), r.input]),
      [
        [`skills remove ${NAME}`, ""],
        [`skills install ${NAME}`, SKILL_TEXT],
      ],
    );
    const calls = [];
    enabled = false;
    await syncBuiltinSkillsOnHost(
      { exec: async (e, command) => calls.push(command) },
      "ssh:u@h",
      {},
    );
    assert.deepEqual(calls, [
      `"$HOME/.sushiai/bin/sushiai" skills remove ${NAME}`,
    ]);
  } finally {
    configureArtifactsSkill({ isEnabled: () => true });
  }
});

test("the artifacts manifest validates as a builtin", () => {
  const manifest = validateExtensionManifest(
    structuredClone(ARTIFACTS_MANIFEST),
  );
  assert.equal(manifest.id, "builtin.artifacts");
  assert.deepEqual(manifest.contributions.surfaces[0].allowedHosts, [
    "workspace.pane",
  ]);
  assert.equal(manifest.contributions.surfaces[0].instancePolicy, "multiple");
});

test("a non-builtin copy with the core view is rejected", () => {
  const copy = structuredClone(ARTIFACTS_MANIFEST);
  copy.id = "acme.artifacts";
  copy.source = { kind: "local" };
  assert.throws(() => validateExtensionManifest(copy), /core views|source/i);
});

test("the artifacts builtin can be turned off", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "artifacts-manager-"));
  try {
    const manager = new ExtensionManager({
      dataDir: dir,
      builtins: [ARTIFACTS_MANIFEST],
      localDir: path.join(dir, "local"),
    });
    await manager.init?.();
    assert.equal(manager.isEnabled("builtin.artifacts"), true);
    await manager.setEnabled("builtin.artifacts", false);
    assert.equal(manager.isEnabled("builtin.artifacts"), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("the skill teaches sushiai open with an absolute path", () => {
  const text = fs.readFileSync(
    path.join(__dirname, "..", "electron", "extensions", "artifacts-skill.md"),
    "utf8",
  );
  assert.match(text, /sushiai open artifacts\/preview "\$\(pwd\)\/artifacts\//);
  assert.match(text, /SUSHIAI_SESSION_ID/);
});
