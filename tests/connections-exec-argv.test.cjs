const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");

const { Connections } = require("../electron/connections.cjs");

async function rig(t, script) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-argv-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const ssh = path.join(dir, "ssh");
  await fs.writeFile(ssh, `#!/bin/sh\n${script}\n`, { mode: 0o755 });
  const connections = new Connections(dir, { ssh });
  const profile = await connections.save({
    id: "00000000-0000-4000-8000-000000000001",
    name: "Devbox",
    host: "user@devbox",
  });
  return { dir, connections, endpoint: `ssh:${profile.id}` };
}

test("execArgv quotes each word, passes stdin bytes, and returns the code and both tails", async (t) => {
  const { dir, connections, endpoint } = await rig(
    t,
    `for last; do :; done
printf '%s' "$last" > "$(dirname "$0")/command"
cat > "$(dirname "$0")/stdin"
i=0; while [ $i -lt 3000 ]; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; i=$((i+1)); done
printf 'END-OUT'
printf 'ERR-LINE' >&2
exit 7`,
  );
  const input = Buffer.from([0, 1, 2, 255, 254, 10, 13]);
  const result = await connections.execArgv(
    endpoint,
    ["sh", "-c", "echo 'it''s'; $HOME"],
    { input },
  );
  assert.equal(result.code, 7);
  assert.equal(Buffer.byteLength(result.stdout), 65536);
  assert.ok(result.stdout.endsWith("END-OUT"));
  assert.equal(result.stderr, "ERR-LINE");
  // The command is fixed text plus base64: no quote of the login shell is
  // involved, whatever the words hold.
  const command = await fs.readFile(path.join(dir, "command"), "utf8");
  assert.match(
    command,
    /^exec \/bin\/sh -c 'eval "\$\(printf %s [A-Za-z0-9+/=]+ \| /,
  );
  assert.deepEqual(await fs.readFile(path.join(dir, "stdin")), input);
});

test("execArgv reports code null when the timeout kills the run", async (t) => {
  const { connections, endpoint } = await rig(t, "sleep 5");
  const result = await connections.execArgv(endpoint, ["true"], {
    timeout: 100,
  });
  assert.equal(result.code, null);
  assert.match(result.stderr, /Timed out/);
});

test("execArgv refuses a host that connects through a command", async (t) => {
  const { connections } = await rig(t, "true");
  const profile = await connections.save({
    id: "00000000-0000-4000-8000-000000000002",
    name: "Cmd",
    connector: { kind: "command", argv: ["tool"] },
  });
  await assert.rejects(
    () => connections.execArgv(`ssh:${profile.id}`, ["true"]),
    /no shell access/,
  );
});

const HOSTILE = [
  "plain",
  "it's",
  'say "hi"',
  "back\\slash \\' \\\\",
  "$HOME `id` $(id) ; | & > <",
  "two\nlines\n",
  "tab\there",
  "  spaces  ",
  "",
  "\u202eRLO\u200bzw",
  "unicode \u00e9\u65e5\u672c",
  "!history ~tilde *glob? {a,b}",
];

test("every login shell hands the program exactly the argv", async (t) => {
  const { remoteCommand } = require("../electron/ssh-command.cjs");
  const { spawnSync } = require("node:child_process");
  const argv = [
    "sh",
    "-c",
    'for a; do printf "%s\\0" "$a"; done',
    "name",
    ...HOSTILE,
  ];
  const command = remoteCommand(argv);
  const shells = ["sh", "bash", "zsh", "fish", "dash", "ksh"].filter(
    (shell) => spawnSync("which", [shell]).status === 0,
  );
  assert.ok(shells.includes("sh") && shells.includes("bash"));
  for (const shell of shells) {
    // Login shells run `shell -c <command>`, which is what sshd does.
    const result = spawnSync(shell, ["-c", command], { encoding: "utf8" });
    assert.equal(result.status, 0, `${shell}: ${result.stderr}`);
    assert.deepEqual(
      result.stdout.split("\0").slice(0, -1),
      HOSTILE,
      `${shell} must pass the same words`,
    );
  }
  t.diagnostic(`shells tried: ${shells.join(", ")}`);
});

test("stdin reaches the program untouched through the wrapper", () => {
  const { remoteCommand } = require("../electron/ssh-command.cjs");
  const { spawnSync } = require("node:child_process");
  const input = Buffer.from([0, 1, 2, 255, 10, 13, 39]);
  const result = spawnSync("sh", ["-c", remoteCommand(["cat"])], { input });
  assert.deepEqual(result.stdout, input);
});

test("a command too long for one argument is refused", () => {
  const { remoteCommand } = require("../electron/ssh-command.cjs");
  assert.throws(() => remoteCommand(["x".repeat(80000)]), /too long/);
});
