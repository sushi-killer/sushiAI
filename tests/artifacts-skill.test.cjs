const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");
const {
  ARTIFACTS_MANIFEST,
} = require("../electron/extensions/builtin-artifacts.cjs");
const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");
const {
  installLocalArtifactsSkill,
  installRemoteArtifactsSkill,
  remoteInstallScript,
} = require("../electron/artifacts-skill.cjs");

const TEXT = "Skill \"quoted\" $HOME `tick` 'single'\nline two\n";

function tempHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "artifacts-skill-"));
}

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

test("local install writes Claude Code, Codex and every Codex account home, idempotently", () => {
  const home = tempHome();
  try {
    const account = path.join(home, "accounts", "a1");
    const first = installLocalArtifactsSkill(home, TEXT, [account]);
    const files = [
      path.join(home, ".claude/skills/sushiai-artifacts/SKILL.md"),
      path.join(home, ".codex/skills/sushiai-artifacts/SKILL.md"),
      path.join(account, "skills/sushiai-artifacts/SKILL.md"),
    ];
    assert.deepEqual(first, files);
    for (const file of files) assert.equal(fs.readFileSync(file, "utf8"), TEXT);
    assert.equal(fs.existsSync(path.join(home, ".agents")), false);
    assert.deepEqual(installLocalArtifactsSkill(home, TEXT, [account]), []);
    assert.deepEqual(
      installLocalArtifactsSkill(home, TEXT + "x", [account]),
      files,
    );
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the remote script round-trips the exact content through a fake exec", async () => {
  const home = tempHome();
  const calls = [];
  try {
    fs.mkdirSync(path.join(home, ".codex"));
    const exec = async (endpoint, command, { input }) => {
      calls.push({ endpoint, command });
      return execFileSync("/bin/sh", ["-s"], {
        input,
        env: { ...process.env, HOME: home },
      }).toString();
    };
    const saved = process.env.SUSHIAI_TEST_WINDOW;
    delete process.env.SUSHIAI_TEST_WINDOW;
    try {
      const ok = await installRemoteArtifactsSkill(
        { exec },
        "ssh:user@devbox",
        {},
      );
      assert.equal(ok, true);
    } finally {
      if (saved !== undefined) process.env.SUSHIAI_TEST_WINDOW = saved;
    }
    assert.equal(calls[0].endpoint, "ssh:user@devbox");
    const file = path.join(home, ".codex/skills/sushiai-artifacts/SKILL.md");
    assert.equal(
      fs.readFileSync(file, "utf8"),
      fs.readFileSync(
        path.join(__dirname, "../electron/extensions/artifacts-skill.md"),
        "utf8",
      ),
    );
    assert.ok(
      fs.existsSync(
        path.join(home, ".claude/skills/sushiai-artifacts/SKILL.md"),
      ),
    );
    assert.equal(fs.existsSync(path.join(home, ".agents")), false);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the remote script decodes to the exact text, hostile characters included", () => {
  const home = tempHome();
  try {
    fs.mkdirSync(path.join(home, ".claude"));
    execFileSync("/bin/sh", ["-s"], {
      input: remoteInstallScript(TEXT),
      env: { ...process.env, HOME: home },
    });
    assert.equal(
      fs.readFileSync(
        path.join(home, ".claude/skills/sushiai-artifacts/SKILL.md"),
        "utf8",
      ),
      TEXT,
    );
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("a failing host exec never throws", async () => {
  const saved = process.env.SUSHIAI_TEST_WINDOW;
  delete process.env.SUSHIAI_TEST_WINDOW;
  const warn = console.warn;
  console.warn = () => {};
  try {
    const exec = async () => {
      throw new Error("unreachable 192.0.2.7");
    };
    assert.equal(
      await installRemoteArtifactsSkill({ exec }, "ssh:user@devbox", {}),
      false,
    );
  } finally {
    console.warn = warn;
    if (saved !== undefined) process.env.SUSHIAI_TEST_WINDOW = saved;
  }
});
