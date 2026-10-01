const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  makeHost,
  makeStore,
  registerHandlers,
} = require("./helpers/fake-host.cjs");
const {
  sshRemote,
  sshFallback,
  gitFailure,
} = require("../electron/project-git-ssh.cjs");

const GIT = [
  "#!/bin/sh",
  'if [ "$1" = clone ]; then',
  '  printf "%s\\n" "$@" >> "$HOME/clone-argv"',
  '  for arg do case "$arg" in https://*)',
  "    for destination do :; done",
  '    mkdir -p "$destination/.git"',
  '    echo "fatal: could not read Username for https://git.example.test: terminal prompts disabled" >&2',
  "    exit 128;; esac; done",
  "fi",
  'exec REAL_GIT "$@"',
].join("\n");
const SSH = [
  "#!/bin/sh",
  'printf "%s\\n" "$GIT_SSH_COMMAND" >> "$HOME/git-ssh-command"',
  'printf "%s\\n" "$@" >> "$HOME/git-ssh-argv"',
  'case "$GIT_SSH_COMMAND" in *"BatchMode=yes"*"StrictHostKeyChecking=yes"*) ;; *) echo "missing strict SSH options" >&2; exit 1;; esac',
  'known="$HOME/.ssh/known_hosts"',
  'case "$GIT_SSH_COMMAND" in *"GlobalKnownHostsFile=/dev/null"*) known="$HOME/.sushiai/git/known_hosts";; esac',
  'server_key=$(cat "$HOME/server.pub")',
  'if [ ! -f "$known" ]; then echo "Host key verification failed." >&2; exit 255; fi',
  'if ! grep -F "$server_key" "$known" >/dev/null; then echo "REMOTE HOST IDENTIFICATION HAS CHANGED!" >&2; exit 255; fi',
  'if [ ! -f "$HOME/authorized" ]; then echo "Permission denied (publickey)." >&2; exit 255; fi',
  'if [ -f "$HOME/.sushiai/git/id_ed25519" ]; then',
  '  ssh-keygen -y -P "" -f "$HOME/.sushiai/git/id_ed25519" | grep -F "$(cat "$HOME/authorized")" >/dev/null || { echo "Permission denied (publickey)." >&2; exit 255; }',
  "fi",
  'exec REAL_GIT upload-pack "$HOME/origin"',
].join("\n");
const SCAN = [
  "#!/bin/sh",
  "port=22; host=",
  'while [ "$#" -gt 0 ]; do case "$1" in -p) port="$2"; shift 2;; -T) shift 2;; *) host="$1"; shift;; esac; done',
  '[ "$port" = 22 ] || host="[$host]:$port"',
  'printf "%s %s\\n" "$host" "$(cat "$HOME/server.pub")"',
].join("\n");

async function rig(
  t,
  {
    bin = {},
    url = "https://git.example.test/team/app.git",
    branch = "main",
    setup,
  } = {},
) {
  const host = await makeHost(t, {
    bin: { git: GIT, ssh: SSH, "ssh-keyscan": SCAN, ...bin },
  });
  const origin = path.join(host.home, "origin");
  execFileSync("git", ["init", "-q", "-b", "main", origin]);
  await fs.writeFile(path.join(origin, "README.md"), "Synthetic source\n");
  execFileSync("git", ["-C", origin, "add", "."]);
  execFileSync("git", [
    "-C",
    origin,
    "-c",
    "user.name=Dev",
    "-c",
    "user.email=dev@example.invalid",
    "commit",
    "-qm",
    "initial",
  ]);
  const serverKey = path.join(host.root, "server-key");
  execFileSync("ssh-keygen", [
    "-q",
    "-t",
    "ed25519",
    "-N",
    "",
    "-f",
    serverKey,
  ]);
  const serverPublic = (await fs.readFile(serverKey + ".pub", "utf8"))
    .trim()
    .split(" ")
    .slice(0, 2)
    .join(" ");
  await fs.writeFile(path.join(host.home, "server.pub"), serverPublic);
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    git: { url, defaultBranch: branch },
    ...(setup ? { setup } : {}),
  });
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  const prepare = (options = {}) =>
    call("projects:host:prepare", project.id, host.endpoint, false, options);
  const scan = (gitUrl = "git@git.example.test:team/app.git") =>
    call("projects:host:git-scan", project.id, host.endpoint, gitUrl);
  const trust = (scanId) =>
    call("projects:host:git-trust", project.id, host.endpoint, scanId);
  return {
    host,
    project,
    projects,
    call,
    prepare,
    scan,
    trust,
    serverPublic,
    origin,
  };
}

async function advanceOrigin(rig, content) {
  await fs.writeFile(path.join(rig.origin, "README.md"), content);
  execFileSync("git", [
    "-C",
    rig.origin,
    "-c",
    "user.name=Dev",
    "-c",
    "user.email=dev@example.invalid",
    "commit",
    "-am",
    "update",
  ]);
}

test("SSH retry validation rejects invalid input before making any host command", async (t) => {
  const r = await rig(t);
  const before = await fs.readFile(r.host.log, "utf8");
  for (const url of [
    "",
    "https://git.example.test/team/app.git",
    "ssh://git@git.example.test:70000/team/app.git",
    "git@other.example.test:team/app.git",
    "git@git.example.test:team/different.git",
  ]) {
    await assert.rejects(
      r.call("projects:host:git-validate", r.project.id, r.host.endpoint, url),
      /SSH|same repository/,
    );
  }
  await r.call(
    "projects:host:git-validate",
    r.project.id,
    r.host.endpoint,
    "ssh://git@git.example.test:2222/team/app.git",
  );
  await assert.rejects(
    r.call(
      "projects:host:git-validate",
      r.project.id,
      "ssh:unknown",
      "git@git.example.test:team/app.git",
    ),
  );
  assert.equal(await fs.readFile(r.host.log, "utf8"), before);
});

test("HTTPS failure falls back to strict SSH, then scan, key and retry clone without an interactive login", async (t) => {
  const r = await rig(t);
  const occupied = path.join(r.host.home, "sushiai", "app.prepare");
  await fs.mkdir(occupied, { recursive: true });
  await fs.writeFile(path.join(occupied, "notes"), "keep");
  let result = await r.prepare();
  assert.equal(result.ok, false);
  assert.equal(result.git.kind, "host-key");
  assert.equal(result.git.transport, "ssh");
  assert.equal(result.git.url, "git@git.example.test:team/app.git");
  assert.equal(await fs.readFile(path.join(occupied, "notes"), "utf8"), "keep");
  assert.deepEqual(await fs.readdir(path.dirname(occupied)), ["app.prepare"]);
  const scan = await r.scan();
  assert.equal(scan.host, "git.example.test");
  assert.match(scan.fingerprints[0], /^SHA256:/);
  assert.equal("keys" in scan, false);
  await r.trust(scan.scanId);
  await assert.rejects(r.trust(scan.scanId), /expired|another/);
  result = await r.prepare({ gitUrl: result.git.url });
  assert.equal(result.git.kind, "auth");
  const key = await r.call(
    "projects:host:git-key",
    r.project.id,
    r.host.endpoint,
  );
  assert.match(key.publicKey, /^ssh-ed25519 /);
  assert.match(key.fingerprint, /^SHA256:/);
  assert.doesNotMatch(JSON.stringify(key), /PRIVATE KEY/);
  const privateFile = path.join(r.host.home, ".sushiai/git/id_ed25519");
  const original = await fs.readFile(privateFile);
  assert.equal((await fs.stat(privateFile)).mode & 0o777, 0o600);
  await fs.rm(privateFile + ".pub");
  assert.deepEqual(
    await r.call("projects:host:git-key", r.project.id, r.host.endpoint),
    key,
  );
  assert.deepEqual(await fs.readFile(privateFile), original);
  await fs.writeFile(path.join(r.host.home, "authorized"), key.publicKey);
  result = await r.prepare({ gitUrl: result.git.url });
  assert.equal(result.ok, true, result.message);
  assert.equal(
    await fs.readFile(path.join(result.path, "README.md"), "utf8"),
    "Synthetic source\n",
  );
  assert.equal(
    execFileSync("git", ["-C", result.path, "remote", "get-url", "origin"], {
      encoding: "utf8",
    }).trim(),
    "git@git.example.test:team/app.git",
  );
  assert.equal((await r.projects.get(r.project.id)).git.url, r.project.git.url);
  assert.deepEqual(await fs.readdir(path.dirname(occupied)), [
    "app",
    "app.prepare",
  ]);
  await advanceOrigin(r, "Updated over SSH\n");
  result = await r.prepare();
  assert.equal(result.ok, true, result.message);
  assert.equal(result.pull, "updated");
  assert.equal(
    await fs.readFile(path.join(result.path, "README.md"), "utf8"),
    "Updated over SSH\n",
  );
  assert.deepEqual(await fs.readFile(privateFile), original);
  assert.equal((await r.projects.get(r.project.id)).git.url, r.project.git.url);
});

test("SSH override retains the same repository and sends a custom port to Git and the host-key scan", async (t) => {
  const r = await rig(t);
  const url = "ssh://git@git.example.test:2222/team/app.git";
  const scan = await r.scan(url);
  assert.equal(scan.host, "[git.example.test]:2222");
  await r.trust(scan.scanId);
  const key = await r.call(
    "projects:host:git-key",
    r.project.id,
    r.host.endpoint,
  );
  await fs.writeFile(path.join(r.host.home, "authorized"), key.publicKey);
  let result = await r.prepare({ gitUrl: url });
  assert.equal(result.ok, true, result.message);
  assert.match(
    await fs.readFile(path.join(r.host.home, "git-ssh-argv"), "utf8"),
    /-p\n2222/,
  );
  await fs.writeFile(path.join(r.host.home, "git-ssh-argv"), "");
  await advanceOrigin(r, "Custom port update\n");
  result = await r.prepare();
  assert.equal(result.ok, true, result.message);
  assert.equal(result.pull, "updated");
  assert.equal(
    await fs.readFile(path.join(result.path, "README.md"), "utf8"),
    "Custom port update\n",
  );
  assert.match(
    await fs.readFile(path.join(r.host.home, "git-ssh-argv"), "utf8"),
    /-p\n2222/,
  );
  const previousHead = execFileSync(
    "git",
    ["-C", result.path, "rev-parse", "HEAD"],
    { encoding: "utf8" },
  );
  await advanceOrigin(r, "Not pulled\n");
  const sshCalls = await fs.readFile(
    path.join(r.host.home, "git-ssh-argv"),
    "utf8",
  );
  result = await r.prepare({ pull: false });
  assert.equal(result.ok, true, result.message);
  assert.equal(result.pull, "skipped:not-asked");
  assert.equal(
    execFileSync("git", ["-C", result.path, "rev-parse", "HEAD"], {
      encoding: "utf8",
    }),
    previousHead,
  );
  assert.equal(
    await fs.readFile(path.join(r.host.home, "git-ssh-argv"), "utf8"),
    sshCalls,
  );
  await assert.rejects(
    r.prepare({ gitUrl: "git@other.example.test:team/app.git" }),
    /same repository/,
  );
  await assert.rejects(
    r.prepare({ gitUrl: "ssh://git@git.example.test:70000/team/app.git" }),
    /valid SSH/,
  );
});

test("HTTP clone timeouts retain HTTPS recovery context and do not launch a second unbounded attempt", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    git: {
      url: "https://git.example.test/team/app.git",
      defaultBranch: "main",
    },
  });
  let attempts = 0;
  const call = registerHandlers({
    projects,
    connections: {
      exec: async (_host, script) => {
        if (script.includes("ABSENT")) return "ABSENT\n";
        if (script.includes("FOUND=")) return "";
        attempts++;
        throw Object.assign(new Error("Connection timed out"), {
          timedOut: true,
          stderr: "SUSHIAI_STEP=start:1\nSUSHIAI_STAGE=clone\n",
        });
      },
    },
    claudeMcp: {},
  });
  const result = await call("projects:host:prepare", project.id, "ssh:devbox");
  assert.equal(result.ok, false);
  assert.equal(result.timedOut, true);
  assert.equal(result.stage, "clone");
  assert.equal(result.git.kind, "network");
  assert.equal(result.git.transport, "https");
  assert.equal(result.git.sshUrl, "git@git.example.test:team/app.git");
  assert.equal(attempts, 1);
});

test("host key replacement requires a fresh preview and preserves user trust and unrelated app entries", async (t) => {
  const r = await rig(t);
  const otherKey = path.join(r.host.root, "old-key");
  execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", otherKey]);
  const oldPublic = (await fs.readFile(otherKey + ".pub", "utf8"))
    .trim()
    .split(" ")
    .slice(0, 2)
    .join(" ");
  const userFile = path.join(r.host.home, ".ssh/known_hosts");
  await fs.mkdir(path.dirname(userFile));
  await fs.writeFile(userFile, "git.example.test " + oldPublic + "\n");
  const ownFile = path.join(r.host.home, ".sushiai/git/known_hosts");
  await fs.mkdir(path.dirname(ownFile), { recursive: true });
  await fs.writeFile(
    ownFile,
    "unrelated.example.test " +
      oldPublic +
      "\ngit.example.test " +
      oldPublic +
      "\n",
  );
  const before = await fs.readFile(userFile);
  const failure = await r.prepare();
  assert.equal(failure.git.changed, true);
  const scan = await r.scan();
  assert.equal(scan.changed, true);
  assert.equal(
    await fs.readFile(ownFile, "utf8"),
    "unrelated.example.test " +
      oldPublic +
      "\ngit.example.test " +
      oldPublic +
      "\n",
  );
  await r.trust(scan.scanId);
  assert.deepEqual(await fs.readFile(userFile), before);
  assert.equal(
    await fs.readFile(ownFile, "utf8"),
    "unrelated.example.test " +
      oldPublic +
      "\ngit.example.test " +
      r.serverPublic +
      "\n",
  );
  await fs.writeFile(path.join(r.host.home, "authorized"), "existing-agent");
  const result = await r.prepare({ gitUrl: failure.git.url });
  assert.equal(result.ok, true, result.message);
  assert.equal((await fs.stat(ownFile)).mode & 0o777, 0o600);
  const command = await fs.readFile(
    path.join(r.host.home, "git-ssh-command"),
    "utf8",
  );
  assert.match(command, /GlobalKnownHostsFile=\/dev\/null/);
});

test("scan trust tokens are bound to the project, endpoint and unchanged repository and trust preview", async (t) => {
  const r = await rig(t);
  const scan = await r.scan();
  await assert.rejects(
    r.call("projects:host:git-trust", r.project.id, "ssh:other", scan.scanId),
    /another/,
  );
  const other = await r.projects.upsert({
    name: "Other",
    git: { url: "https://git.example.test/team/other.git" },
  });
  await assert.rejects(
    r.call("projects:host:git-trust", other.id, r.host.endpoint, scan.scanId),
    /another/,
  );
  await fs.mkdir(path.join(r.host.home, ".ssh"));
  await fs.writeFile(
    path.join(r.host.home, ".ssh/known_hosts"),
    "git.example.test " + r.serverPublic + "\n",
  );
  await assert.rejects(r.trust(scan.scanId), /trust files changed/);
  const fresh = await r.scan();
  await r.projects.upsert({
    ...r.project,
    git: {
      ...r.project.git,
      url: "https://git.example.test/team/different.git",
    },
  });
  await assert.rejects(r.trust(fresh.scanId), /another/);
  await assert.rejects(
    r.call("projects:host:git-key", "__proto__", r.host.endpoint),
    /no git remote/,
  );
  await assert.rejects(
    r.call(
      "projects:host:git-scan",
      r.project.id,
      r.host.endpoint,
      "https://git.example.test/team/app.git",
    ),
    /SSH git URL/,
  );
});

test("branch and filesystem failures do not retry over SSH, and setup failures stay setup failures", async (t) => {
  for (const [message, kind] of [
    ["fatal: Remote branch missing not found in upstream origin", "branch"],
    ["fatal: could not create work tree dir: No space left on device", "path"],
  ]) {
    const r = await rig(t, {
      bin: {
        git:
          '#!/bin/sh\nif [ "$1" = clone ]; then echo attempted >> "$HOME/attempts"; echo ' +
          JSON.stringify(message) +
          ' >&2; exit 128; fi\nexec REAL_GIT "$@"\n',
      },
    });
    const result = await r.prepare();
    assert.equal(result.git.kind, kind);
    assert.equal(
      (await fs.readFile(path.join(r.host.home, "attempts"), "utf8")).trim(),
      "attempted",
    );
  }
  const r = await rig(t, {
    setup: { install: "echo '403 Forbidden' >&2; exit 1" },
  });
  const scan = await r.scan();
  await r.trust(scan.scanId);
  await fs.writeFile(path.join(r.host.home, "authorized"), "existing-agent");
  const result = await r.prepare();
  assert.equal(result.stage, "setup");
  assert.equal(result.git, undefined);
  assert.equal(result.status, undefined);
});

test("invalid SSH input never becomes a shell command and corrupt existing private keys are not replaced", async (t) => {
  const r = await rig(t);
  for (const url of [
    "-oProxyCommand=touch bad",
    "https://git.example.test/team/app.git",
    "ssh://git@-bad/team/app",
    "ssh://git:password@git.example.test/team/app.git",
    "git@git.example.test:team/app.git\n;touch bad",
  ]) {
    await assert.rejects(r.scan(url), /SSH|password/);
  }
  const keyPath = path.join(r.host.home, ".sushiai/git/id_ed25519");
  await fs.mkdir(path.dirname(keyPath), { recursive: true });
  await fs.writeFile(keyPath, "corrupt key\n");
  await assert.rejects(
    r.call("projects:host:git-key", r.project.id, r.host.endpoint),
    /invalid format|error in libcrypto/,
  );
  assert.equal(await fs.readFile(keyPath, "utf8"), "corrupt key\n");
  await fs.rm(keyPath);
  const mine = path.join(r.host.home, "private-mine");
  await fs.writeFile(mine, "preserve\n");
  await fs.symlink(mine, keyPath);
  await assert.rejects(
    r.call("projects:host:git-key", r.project.id, r.host.endpoint),
    /symbolic link/,
  );
  assert.equal(await fs.readFile(mine, "utf8"), "preserve\n");
});

test("Git errors classify transport failures and derive SSH without carrying HTTP credentials or ports", () => {
  assert.equal(
    sshFallback("https://user:password@git.example.test:8443/team/app.git"),
    "git@git.example.test:team/app.git",
  );
  assert.equal(sshFallback("/local/folder"), undefined);
  assert.throws(
    () => sshRemote("https://git.example.test/team/app.git"),
    /SSH git URL/,
  );
  assert.equal(
    sshFallback("https://git.example.test/team/app.git?token=secret"),
    undefined,
  );
  assert.deepEqual(sshRemote("ssh://git@[2001:db8::1]:2222/team/app.git"), {
    url: "ssh://git@[2001:db8::1]:2222/team/app.git",
    hostname: "2001:db8::1",
    port: 2222,
    knownHost: "[2001:db8::1]:2222",
  });
  assert.equal(
    gitFailure("Connection refused", "git@git.example.test:team/app.git").kind,
    "network",
  );
  assert.equal(
    gitFailure(
      "Permission denied (publickey).",
      "git@git.example.test:team/app.git",
    ).kind,
    "auth",
  );
  assert.equal(
    gitFailure(
      "403 Forbidden",
      "https://user:password@git.example.test/team/app.git?token=secret#fragment",
    ).url,
    "https://git.example.test/team/app.git",
  );
  assert.equal(gitFailure("403 Forbidden", "/local/folder"), undefined);
});
