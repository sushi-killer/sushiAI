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

test("need_auth offers retry and the ssh command with the port", async () => {
  const { describeHost } = await load();
  const v = describeHost(st("need_auth", { message: "Password needed" }), {
    host: "user@devbox",
    port: 2222,
  });
  assert.equal(v.action, "retry");
  assert.equal(v.detail, "Password needed");
  assert.equal(v.command, "ssh -p 2222 user@devbox");
});

test("host key change has no action and names the known_hosts entry", async () => {
  const { describeHost } = await load();
  const v = describeHost(st("failed", { reason: "host_key_changed" }), profile);
  assert.equal(v.tone, "danger");
  assert.equal(v.action, undefined);
  assert.equal(v.command, "ssh-keygen -R devbox");
  const p = describeHost(st("failed", { reason: "host_key_changed" }), {
    host: "devbox",
    port: 2222,
  });
  assert.equal(p.command, "ssh-keygen -R '[devbox]:2222'");
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

test("a hostile host name is quoted in the suggested command", async () => {
  const { describeHost } = await load();
  const v = describeHost(st("need_auth"), { host: "a b;rm -rf" });
  assert.equal(v.command, "ssh 'a b;rm -rf'");
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
