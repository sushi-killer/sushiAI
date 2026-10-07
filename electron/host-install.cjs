// Installs the `sushiai` binary on a host over an injected exec, and holds the
// probe/upload helpers host-setup.cjs shares.
//
// API (called by host-setup.cjs):
//   installSushiai({ exec, manifest, binDir, upload? }) -> Promise<{
//     status: "installed" | "unchanged", version, path, platform, sha256 }>
//
//   exec(command, { input?, timeout? }) -> Promise<string>   stdout of the
//       command line the host's login shell runs (ssh: `$SHELL -c command`);
//       rejects when it exits non-zero. Bind the endpoint before passing it:
//       (command, o) => connections.exec(ep, command, o). installSushiai
//       wraps every script as `sh -c '<script>'` itself (posixExec), so a
//       fish login shell never parses the POSIX script (csh/tcsh still fail:
//       they reject a newline inside quotes).
//   manifest   parsed target/host/manifest.json, keyed by `uname -sm`
//       ("Linux x86_64", "Linux aarch64", "Darwin arm64", ...) with
//       { target, path, version, size, sha256 }.
//   binDir     directory the manifest `path` values are relative to (target/host).
//   upload(command, buffer, { timeout }) optional, given the already wrapped
//       command line; defaults to exec(command, { input: buffer }), i.e. the bytes travel over the ssh stdin.
//
// Steps: probe `uname -sm` and $HOME, pick the manifest entry, skip the upload
// when ~/.sushiai/versions/<sha256>/sushiai already has the right hash, else
// upload to a temp file in that directory, verify the sha256 on the host,
// chmod 755, atomic mv; then `ln -sfn` it as ~/.sushiai/bin/sushiai (bin dir
// 0700) and run `~/.sushiai/bin/sushiai hooks install`. Errors are plain
// Error messages: unknown platform, missing manifest entry, hash mismatch,
// upload failure. There is no rustup path.
const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { quote } = require("./connections.cjs");

const REMOTE_BIN = "$HOME/.sushiai/bin";
const REMOTE_VERSIONS = "$HOME/.sushiai/versions";
// A non-interactive ssh shell often lacks the user's tool directories.
const REMOTE_PATH =
  'export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"';
const UPLOAD_TIMEOUT_MS = 5 * 60 * 1000;
const PROBE_TIMEOUT_MS = 30 * 1000;
const HOOKS_TIMEOUT_MS = 60 * 1000;

/** One `key=value` per line, as the probe scripts print them. */
function parseProbe(output) {
  const values = {};
  for (const line of String(output).split("\n")) {
    const match = /^([a-z_]+)=(.*)$/.exec(line.trim());
    if (match) values[match[1]] = match[2];
  }
  return values;
}

/** The command line that runs `script` under POSIX sh, whatever the login
 * shell is: sshd hands the command to `$SHELL -c`. */
function posixCommand(script) {
  return `sh -c ${quote(script)}`;
}

/** `exec` with every script wrapped by posixCommand. */
function posixExec(exec) {
  return (script, options) => exec(posixCommand(script), options);
}

function sha256Hex(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

// Prints the sha256 of the file "$1" (read from stdin) with whichever tool
// the host has; the guard prints `nohash=1` and stops when it has none.
const HASH_FUNCTION = `h() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum < "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 < "$1" | cut -d' ' -f1
  else openssl dgst -sha256 < "$1" | sed 's/.*= *//'
  fi
}
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1 && ! command -v openssl >/dev/null 2>&1; then echo "nohash=1"; exit 0; fi`;

const PLATFORM_SCRIPT = `${REMOTE_PATH}
echo "home=$HOME"
echo "platform=$(uname -sm)"
exit 0`;

function checkScript(sha256) {
  return `${REMOTE_PATH}
${HASH_FUNCTION}
f="${REMOTE_VERSIONS}/${sha256}/sushiai"
if [ -x "$f" ] && [ "$(h "$f")" = "${sha256}" ]; then echo "present=1"; fi
exit 0`;
}

// Streams stdin to a temp file beside the final one, so the rename stays on
// one filesystem and a half-written file is never the live binary.
function uploadScript(sha256) {
  return `set -e
${HASH_FUNCTION}
d="${REMOTE_VERSIONS}/${sha256}"
mkdir -p "$d"
t="$d/sushiai.upload.$$"
trap 'rm -f "$t"' EXIT HUP INT TERM PIPE
rm -f "$d"/sushiai.upload.*
cat > "$t"
actual="$(h "$t")"
if [ "$actual" != "${sha256}" ]; then echo "mismatch=$actual"; exit 0; fi
chmod 755 "$t"
mv "$t" "$d/sushiai"
echo "uploaded=1"`;
}

// Replaces the link atomically (symlink to a temp name, then rename), as
// crates/sushiai-daemon/src/binlink.rs does, and never overwrites a file.
function linkScript(sha256) {
  return `set -e
b="${REMOTE_BIN}"
mkdir -p "$b"
chmod 700 "$b"
link="$b/sushiai"
if [ -e "$link" ] && [ ! -L "$link" ]; then echo "notsymlink=1"; exit 0; fi
tmp="$b/sushiai.tmp.$$"
trap 'rm -f "$tmp"' EXIT HUP INT TERM PIPE
rm -f "$tmp"
ln -s "${REMOTE_VERSIONS}/${sha256}/sushiai" "$tmp"
mv -f "$tmp" "$link"
# The legacy orchd binary is replaced by the daemon module; the daemon stops a
# still-running legacy process on its next start.
rm -f "$b/orchd"
echo "linked=1"`;
}

const HOOKS_SCRIPT = `${REMOTE_PATH}
"${REMOTE_BIN}/sushiai" hooks install`;

function pickEntry(manifest, platform) {
  if (!/^[A-Za-z]+ [A-Za-z0-9_]+$/.test(platform))
    throw new Error(`Unknown host platform "${platform || "(empty)"}".`);
  const entry = manifest?.[platform];
  if (!entry)
    throw new Error(
      `This build of sushiAI has no sushiai binary for ${platform}.`,
    );
  if (!/^[0-9a-f]{64}$/.test(String(entry.sha256)))
    throw new Error(`The manifest entry for ${platform} has no valid sha256.`);
  if (
    typeof entry.path !== "string" ||
    path.isAbsolute(entry.path) ||
    entry.path.split(/[\\/]/).includes("..")
  )
    throw new Error(`The manifest entry for ${platform} has an invalid path.`);
  return entry;
}

const NO_HASH_MESSAGE =
  "The host has none of sha256sum, shasum or openssl, so the sushiai upload cannot be verified.";

async function installSushiai({ exec: rawExec, manifest, binDir, upload }) {
  const exec = posixExec(rawExec);
  const send =
    upload ??
    ((command, input, o) => rawExec(command, { input, timeout: o.timeout }));
  const info = parseProbe(
    await exec(PLATFORM_SCRIPT, { timeout: PROBE_TIMEOUT_MS }),
  );
  if (!info.home || !info.home.startsWith("/"))
    throw new Error("Could not read the home directory on the host.");
  const platform = info.platform || "";
  const entry = pickEntry(manifest, platform);
  const { sha256 } = entry;

  const checked = parseProbe(
    await exec(checkScript(sha256), { timeout: PROBE_TIMEOUT_MS }),
  );
  if (checked.nohash) throw new Error(NO_HASH_MESSAGE);
  const present = checked.present === "1";
  if (!present) {
    let bytes;
    try {
      bytes = await fs.readFile(path.join(binDir, entry.path));
    } catch (error) {
      throw new Error(
        `Cannot read the sushiai binary for ${platform}: ${error.message}`,
      );
    }
    if (sha256Hex(bytes) !== sha256)
      throw new Error(
        `The bundled sushiai binary for ${platform} does not match its manifest sha256.`,
      );
    let out;
    try {
      out = parseProbe(
        await send(posixCommand(uploadScript(sha256)), bytes, {
          timeout: UPLOAD_TIMEOUT_MS,
        }),
      );
    } catch (error) {
      throw new Error(
        `Uploading sushiai to the host failed (check free disk space): ${tail(error)}`,
      );
    }
    if (out.nohash) throw new Error(NO_HASH_MESSAGE);
    if (out.mismatch)
      throw new Error(
        `The uploaded sushiai did not match its sha256 on the host (expected ${sha256}, got ${out.mismatch}).`,
      );
    if (out.uploaded !== "1")
      throw new Error("Uploading sushiai to the host did not complete.");
  }
  const livePath = `${info.home}/.sushiai/bin/sushiai`;
  const linked = parseProbe(
    await exec(linkScript(sha256), { timeout: PROBE_TIMEOUT_MS }),
  );
  if (linked.notsymlink)
    throw new Error(`${livePath} exists and is not a symlink; left alone.`);
  try {
    await exec(HOOKS_SCRIPT, { timeout: HOOKS_TIMEOUT_MS });
  } catch (error) {
    throw new Error(
      `Installed sushiai ${entry.version}, but "sushiai hooks install" failed: ${tail(error)}`,
    );
  }
  return {
    status: present ? "unchanged" : "installed",
    version: entry.version,
    path: livePath,
    platform,
    sha256,
  };
}

function tail(error) {
  return String(error?.message ?? error)
    .trim()
    .split("\n")
    .slice(-4)
    .join("\n");
}

module.exports = {
  installSushiai,
  parseProbe,
  posixCommand,
  posixExec,
  sha256Hex,
  REMOTE_PATH,
  REMOTE_BIN,
  UPLOAD_TIMEOUT_MS,
};
