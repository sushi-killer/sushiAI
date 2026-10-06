const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn, execFileSync } = require("node:child_process");
const { installSushiai, sha256Hex } = require("../electron/host-install.cjs");

// A "host" is a temp HOME (with a space, a quote and a dollar sign in its
// name) holding a `uname` shim in ~/.local/bin, which the install scripts put
// first on PATH. exec mimics sshd: it runs the command line through the
// login shell, `$SHELL -c`, and that shell is a stand-in for fish or tcsh that
// fails unless the command is one single-quoted `sh -c '...'` word.
function writeShell(dir) {
  const shell = path.join(dir, "fishlike");
  fs.writeFileSync(
    shell,
    `#!/bin/sh
# Not POSIX: only \`sh -c '<script>'\` reaches a real shell.
case "$2" in
  "sh -c '"*"'") ;;
  *) echo "fishlike: Unsupported use of '('/'trap'/'$$'" >&2; exit 127 ;;
esac
exec /bin/sh -c "$2"
`,
    { mode: 0o755 },
  );
  return shell;
}

function fixture(platform, entries = ["Linux x86_64"], { path: pathEnv } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "host-install-"));
  const home = path.join(root, "ho me's $dir");
  const binDir = path.join(root, "dist");
  fs.mkdirSync(path.join(home, ".local", "bin"), { recursive: true });
  fs.mkdirSync(binDir);
  const shim = path.join(home, ".local", "bin", "uname");
  fs.writeFileSync(shim, `#!/bin/sh\necho "${platform}"\n`, { mode: 0o755 });
  const shell = writeShell(root);
  const manifest = {};
  for (const key of entries) {
    const dir = key.replace(/\W+/g, "-");
    fs.mkdirSync(path.join(binDir, dir));
    const bytes = Buffer.from(
      `#!/bin/sh\necho "$@" >> "$HOME/hooks.log"\n# ${key}\n`,
    );
    fs.writeFileSync(path.join(binDir, dir, "sushiai"), bytes);
    manifest[key] = {
      target: dir,
      path: `${dir}/sushiai`,
      version: "0.1.0",
      size: bytes.length,
      sha256: sha256Hex(bytes),
    };
  }
  const commands = [];
  const exec = (command, { input } = {}) => {
    commands.push(command);
    return new Promise((resolve, reject) => {
      const child = spawn(shell, ["-c", command], {
        env: {
          HOME: home,
          SHELL: shell,
          PATH: pathEnv ?? process.env.PATH,
        },
        stdio: ["pipe", "pipe", "pipe"],
      });
      let out = "";
      let err = "";
      child.stdout.on("data", (c) => (out += c));
      child.stderr.on("data", (c) => (err += c));
      child.on("close", (code) =>
        code ? reject(new Error(err || `exit ${code}`)) : resolve(out),
      );
      child.stdin.on("error", () => {});
      child.stdin.end(input);
    });
  };
  return {
    root,
    home,
    binDir,
    manifest,
    exec,
    commands,
    cleanup: () => fs.rmSync(root, { recursive: true, force: true }),
  };
}

for (const platform of ["Linux x86_64", "Linux aarch64", "Darwin arm64"]) {
  test(`installs the ${platform} entry, links it and runs hooks install`, async (t) => {
    const f = fixture(platform, [platform]);
    t.after(f.cleanup);
    const entry = f.manifest[platform];
    const result = await installSushiai(f);
    assert.equal(result.status, "installed");
    assert.equal(result.version, "0.1.0");
    assert.equal(result.path, `${f.home}/.sushiai/bin/sushiai`);
    const live = path.join(f.home, ".sushiai", "versions", entry.sha256);
    assert.equal(
      fs.readlinkSync(path.join(f.home, ".sushiai", "bin", "sushiai")),
      path.join(live, "sushiai"),
    );
    assert.equal(
      sha256Hex(fs.readFileSync(path.join(live, "sushiai"))),
      entry.sha256,
    );
    assert.equal(fs.statSync(path.join(live, "sushiai")).mode & 0o777, 0o755);
    assert.equal(
      fs.statSync(path.join(f.home, ".sushiai", "bin")).mode & 0o777,
      0o700,
    );
    // No temp upload file is left behind.
    assert.deepEqual(fs.readdirSync(live), ["sushiai"]);
    assert.equal(
      fs.readFileSync(path.join(f.home, "hooks.log"), "utf8"),
      "hooks install\n",
    );
  });
}

test("skips the upload when the right hash is already on the host", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  await installSushiai(f);
  let uploads = 0;
  const result = await installSushiai({
    ...f,
    upload: async () => {
      uploads += 1;
      return "uploaded=1";
    },
  });
  assert.equal(result.status, "unchanged");
  assert.equal(uploads, 0);
  // hooks install still runs on an unchanged binary.
  assert.equal(
    fs.readFileSync(path.join(f.home, "hooks.log"), "utf8"),
    "hooks install\nhooks install\n",
  );
});

test("re-uploads when the stored binary has the wrong content", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  await installSushiai(f);
  const sha = f.manifest["Linux x86_64"].sha256;
  fs.writeFileSync(
    path.join(f.home, ".sushiai", "versions", sha, "sushiai"),
    "#!/bin/sh\nexit 0\n",
  );
  const result = await installSushiai(f);
  assert.equal(result.status, "installed");
});

test("rejects a bundled binary that does not match its manifest sha256", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  fs.appendFileSync(path.join(f.binDir, "Linux-x86_64", "sushiai"), "x");
  await assert.rejects(installSushiai(f), /does not match its manifest sha256/);
});

test("rejects bytes that arrive corrupted on the host", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  await assert.rejects(
    installSushiai({
      ...f,
      upload: (command, bytes, o) =>
        f.exec(command, {
          input: Buffer.concat([bytes, Buffer.from("!")]),
          ...o,
        }),
    }),
    /did not match its sha256 on the host/,
  );
  assert.equal(fs.existsSync(path.join(f.home, ".sushiai", "bin")), false);
  assert.equal(fs.existsSync(path.join(f.home, "hooks.log")), false);
});

test("reports a failed upload", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  await assert.rejects(
    installSushiai({
      ...f,
      upload: async () => {
        throw new Error("cat: write error: No space left on device");
      },
    }),
    /Uploading sushiai to the host failed.*No space left/s,
  );
});

test("errors on an unknown platform and on a missing manifest entry", async (t) => {
  const odd = fixture("Plan9 mips64 extra", ["Linux x86_64"]);
  t.after(odd.cleanup);
  await assert.rejects(installSushiai(odd), /Unknown host platform/);
  const missing = fixture("FreeBSD amd64", ["Linux x86_64"]);
  t.after(missing.cleanup);
  await assert.rejects(
    installSushiai(missing),
    /no sushiai binary for FreeBSD amd64/,
  );
});

test("rejects a manifest sha256 or path that is unsafe in a shell", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  const entry = f.manifest["Linux x86_64"];
  await assert.rejects(
    installSushiai({
      ...f,
      manifest: { "Linux x86_64": { ...entry, sha256: '1"; rm -rf ~; "' } },
    }),
    /no valid sha256/,
  );
  await assert.rejects(
    installSushiai({
      ...f,
      manifest: { "Linux x86_64": { ...entry, path: "../x/sushiai" } },
    }),
    /invalid path/,
  );
});

// Skips unless `npm run build:host` produced target/host. The host side gets an
// allowlist (PATH, SHELL, and HOME set to the temp dir), so CODEX_HOME,
// SUSHIAI_HOME and CLAUDE_CONFIG_DIR of the developer never reach the binary.
test("smoke: installs the real target/host binary when it was built", async (t) => {
  const dist = path.join(__dirname, "..", "target", "host");
  const file = path.join(dist, "manifest.json");
  if (!fs.existsSync(file)) return t.skip("target/host not built");
  const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
  const key = `${os.type()} ${os.arch() === "x64" ? "x86_64" : os.arch()}`;
  if (!manifest[key]) return t.skip(`no target/host entry for ${key}`);
  const f = fixture(key, []);
  t.after(f.cleanup);
  // The shim keeps the real uname answer; the real binary runs hooks install
  // against the temp HOME.
  await installSushiai({ ...f, manifest, binDir: dist });
  assert.ok(fs.existsSync(path.join(f.home, ".sushiai", "bin", "sushiai")));
});

test("every remote command is one sh -c word, so a non-POSIX login shell works", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  await installSushiai(f);
  assert.ok(f.commands.length >= 5);
  for (const command of f.commands) assert.match(command, /^sh -c '[\s\S]*'$/);
  // The stand-in login shell rejects a bare script.
  await assert.rejects(f.exec('trap "" EXIT; echo $$'), /Unsupported use/);
});

test("the bin link is replaced atomically and no temp link is left", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  await installSushiai(f);
  const bin = path.join(f.home, ".sushiai", "bin");
  fs.rmSync(path.join(bin, "sushiai"));
  fs.symlinkSync("/nonexistent/old", path.join(bin, "sushiai"));
  await installSushiai(f);
  assert.deepEqual(fs.readdirSync(bin), ["sushiai"]);
  assert.match(
    fs.readlinkSync(path.join(bin, "sushiai")),
    /versions\/[0-9a-f]{64}\/sushiai$/,
  );
  assert.ok(f.commands.some((c) => /mv -f/.test(c) && !/ln -sfn/.test(c)));
});

test("an install removes the legacy orchd binary", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  const bin = path.join(f.home, ".sushiai", "bin");
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(bin, "orchd"), "legacy");
  await installSushiai(f);
  assert.deepEqual(fs.readdirSync(bin), ["sushiai"]);
});

test("a regular file at the link path is never overwritten", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  const bin = path.join(f.home, ".sushiai", "bin");
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(bin, "sushiai"), "mine");
  await assert.rejects(installSushiai(f), /exists and is not a symlink/);
  assert.equal(fs.readFileSync(path.join(bin, "sushiai"), "utf8"), "mine");
  assert.equal(fs.existsSync(path.join(f.home, "hooks.log")), false);
});

test("a failing hooks install is reported after the binary is installed", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  const entry = f.manifest["Linux x86_64"];
  const bytes = Buffer.from('#!/bin/sh\necho "hooks broke" >&2\nexit 3\n');
  fs.writeFileSync(path.join(f.binDir, entry.path), bytes);
  entry.sha256 = sha256Hex(bytes);
  await assert.rejects(
    installSushiai(f),
    /Installed sushiai 0\.1\.0, but "sushiai hooks install" failed: .*hooks broke/s,
  );
});

test("stale upload temp files are removed before a new upload", async (t) => {
  const f = fixture("Linux x86_64");
  t.after(f.cleanup);
  const dir = path.join(
    f.home,
    ".sushiai",
    "versions",
    f.manifest["Linux x86_64"].sha256,
  );
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "sushiai.upload.99999"), "partial");
  await installSushiai(f);
  assert.deepEqual(fs.readdirSync(dir), ["sushiai"]);
});

// A PATH holding only the tools the scripts need plus the chosen hashers.
function restrictedPath(t, hashers) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "host-tools-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const which = (name) =>
    execFileSync("/bin/sh", ["-c", `command -v ${name}`], {
      encoding: "utf8",
    }).trim();
  for (const name of "sh cat mkdir chmod mv rm ln cut sed".split(" "))
    fs.symlinkSync(which(name), path.join(dir, name));
  for (const name of hashers) {
    if (name === "sha256sum")
      fs.writeFileSync(
        path.join(dir, name),
        `#!/bin/sh\nexec ${which("shasum")} -a 256\n`,
        { mode: 0o755 },
      );
    else fs.symlinkSync(which(name), path.join(dir, name));
  }
  return dir;
}

for (const hasher of ["sha256sum", "shasum", "openssl"]) {
  test(`verifies the upload with ${hasher} alone`, async (t) => {
    const f = fixture("Linux x86_64", ["Linux x86_64"], {
      path: restrictedPath(t, [hasher]),
    });
    t.after(f.cleanup);
    assert.equal((await installSushiai(f)).status, "installed");
    assert.equal((await installSushiai(f)).status, "unchanged");
  });
}

test("says so when the host has no sha256 tool", async (t) => {
  const f = fixture("Linux x86_64", ["Linux x86_64"], {
    path: restrictedPath(t, []),
  });
  t.after(f.cleanup);
  await assert.rejects(
    installSushiai(f),
    /none of sha256sum, shasum or openssl/,
  );
});
