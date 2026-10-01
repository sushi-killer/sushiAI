const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { claudeFdLaunch } = require("../electron/ipc/terminals.cjs");

test("subscription launch delivers the token on fd 3 and removes its file", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-claude-fd-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const tokenPath = path.join(dir, "token");
  const cliPath = path.join(dir, "fake-claude");
  await fs.writeFile(tokenPath, "invented-subscription-token", { mode: 0o600 });
  await fs.writeFile(
    cliPath,
    "#!/bin/sh\nprintf 'fd='; cat <&3; printf '\\n'; env\n",
    { mode: 0o700 },
  );
  const launch = claudeFdLaunch(cliPath, tokenPath);
  const result = spawnSync(launch.binary, launch.args, { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^fd=invented-subscription-token\n/m);
  assert.match(result.stdout, /CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3/);
  const environment = result.stdout.slice(result.stdout.indexOf("\n") + 1);
  assert.doesNotMatch(environment, /invented-subscription-token/);
  await assert.rejects(fs.access(tokenPath));
});
