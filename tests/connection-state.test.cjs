const test = require("node:test");
const assert = require("node:assert/strict");

async function load() {
  return import("../src/connectionState.ts");
}
const profile = { host: "user@devbox", port: undefined };
const st = (state, extra = {}) => ({
  host: "h",
  generation: 1,
  state,
  ...extra,
});

test("ready shows the version and no action", async () => {
  const { describeHost } = await load();
  const v = describeHost(st("ready", { version: "0.4.1" }), profile);
  assert.equal(v.label, "Connected");
  assert.equal(v.detail, "sushiai 0.4.1");
  assert.equal(v.action, undefined);
});

test("connecting, offline and missing state", async () => {
  const { describeHost } = await load();
  assert.equal(describeHost(st("connecting"), profile).label, "Connecting");
  assert.equal(
    describeHost(st("offline"), profile).label,
    "Offline, reconnecting",
  );
  assert.equal(describeHost(undefined, profile).label, "Not connected");
});

test("need_auth offers retry and asks for a key or the agent, not a password", async () => {
  const { describeHost } = await load();
  const v = describeHost(st("need_auth", { message: "No key" }), {
    host: "user@devbox",
    port: 2222,
  });
  assert.equal(v.action, "retry");
  assert.equal(v.detail, "No key");
  assert.equal(v.command, undefined);
  assert.match(v.hint, /ssh agent/);
  assert.doesNotMatch(v.hint, /password/i);
});

test("host key change has no action and copies the command the app built", async () => {
  const { describeHost } = await load();
  const hint =
    "ssh-keygen -R '[devbox.example.test]:2222' -f '/data/known_hosts'";
  const v = describeHost(
    st("failed", { reason: "host_key_changed", hint }),
    profile,
  );
  assert.equal(v.tone, "danger");
  assert.equal(v.action, undefined);
  assert.equal(v.command, hint);
  const none = describeHost(
    st("failed", { reason: "host_key_changed" }),
    profile,
  );
  assert.equal(none.command, undefined);
});

test("not installed and incompatible map to install and update", async () => {
  const { describeHost, actionLabel } = await load();
  const n = describeHost(st("failed", { reason: "not_installed" }), profile);
  assert.equal(n.action, "install");
  assert.equal(actionLabel(n.action), "Install sushiai");
  const i = describeHost(st("failed", { reason: "incompatible" }), profile);
  assert.equal(i.action, "update");
  assert.equal(actionLabel(i.action), "Update sushiai");
  const f = describeHost(st("failed", { reason: "daemon_died" }), profile);
  assert.equal(f.action, "retry");
});

test("parseArgv splits on spaces and honours quotes", async () => {
  const { parseArgv } = await load();
  assert.deepEqual(parseArgv("  ssh  -J jump  host "), [
    "ssh",
    "-J",
    "jump",
    "host",
  ]);
  assert.deepEqual(parseArgv(`proxy --name "my host" 'a "b"' c\\ d ""`), [
    "proxy",
    "--name",
    "my host",
    'a "b"',
    "c d",
    "",
  ]);
  assert.deepEqual(parseArgv(""), []);
  assert.throws(() => parseArgv('x "open'), /quote/);
});

test("formatArgv round-trips and connectorFromLine picks the kind", async () => {
  const { parseArgv, formatArgv, connectorFromLine } = await load();
  const argv = ["tsh", "ssh", "my host", 'q"uote', ""];
  assert.deepEqual(parseArgv(formatArgv(argv)), argv);
  assert.deepEqual(connectorFromLine("   "), { kind: "ssh" });
  assert.deepEqual(connectorFromLine("tsh ssh devbox"), {
    kind: "command",
    argv: ["tsh", "ssh", "devbox"],
  });
});

test("a ready host whose binary differs from the bundled one offers Update sushiai", async () => {
  const { describeHost, actionLabel } = await load();
  const same = describeHost(st("ready", { version: "0.4.1" }), profile);
  assert.equal(same.action, undefined);
  const view = describeHost(
    st("ready", { version: "0.4.1", update: true }),
    profile,
  );
  assert.equal(view.label, "Connected");
  assert.equal(view.action, "update");
  assert.equal(actionLabel(view.action), "Update sushiai");
});

test("This Mac incompatible says another build owns the daemon and offers Restart daemon", async () => {
  const { describeHost, actionLabel } = await load();
  const view = describeHost(
    st("failed", { host: "local", reason: "incompatible", message: "x" }),
    profile,
  );
  assert.equal(view.action, "restart");
  assert.match(view.label, /Another sushiAI build owns the daemon/);
  assert.equal(actionLabel("restart"), "Restart daemon");
  // A failed connect on This Mac is a plain Retry, as on any host.
  const failed = describeHost(
    st("failed", { host: "local", reason: "daemon_died" }),
    profile,
  );
  assert.equal(failed.action, "retry");
});
