const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  createHostExec,
  CODES,
} = require("../electron/extensions/companion-exec.cjs");
const {
  createCompanions,
  checkResult,
} = require("../electron/extensions/companion-process.cjs");
const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");

const PROFILES = [
  { id: "h-1", name: "Devbox", host: "user@devbox" },
  { id: "h-2", name: "Edge", host: "192.0.2.7", port: 2200 },
  { id: "h-3", name: "Third", host: "192.0.2.8" },
  { id: "h-4", name: "Fourth", host: "192.0.2.9" },
  { id: "h-5", name: "Fifth", host: "192.0.2.10" },
  {
    id: "h-cmd",
    name: "Cmd",
    host: "cmd",
    connector: { kind: "command", argv: ["x"] },
  },
];
const COMPANION = {
  id: "ext.one",
  name: "Extension one",
  permissions: ["hosts.read", "hosts.exec"],
};
const REQUEST = {
  host: "h-1",
  title: "Install the agent",
  argv: ["uname", "-m"],
};

function setup(options = {}) {
  const log = { asked: [], ran: [], audit: [] };
  let clock = 1_000_000;
  const exec = createHostExec({
    getProfiles: () => PROFILES,
    execOnHost: async (endpoint, argv, opts) => {
      log.ran.push({ endpoint, argv, opts });
      return options.result ?? { code: 0, stdout: "ok\n", stderr: "" };
    },
    askOwner: async (question) => {
      log.asked.push(question);
      return options.answer ?? true;
    },
    audit: (line) => log.audit.push(line),
    now: () => clock,
    ...options.deps,
  });
  return { exec, log, advance: (ms) => (clock += ms) };
}

const rejectsWith = (promise, code) =>
  assert.rejects(promise, (error) => error.code === CODES[code]);

test("host.exec is refused without the hosts.exec permission", async () => {
  const { exec, log } = setup();
  await rejectsWith(
    exec.run({ ...COMPANION, permissions: ["hosts.read"] }, REQUEST),
    "permission",
  );
  assert.equal(log.asked.length + log.ran.length, 0);
});

test("an unknown host, the local host and a command host are refused", async () => {
  const { exec, log } = setup();
  for (const host of ["nope", "local", "h-cmd"])
    await rejectsWith(exec.run(COMPANION, { ...REQUEST, host }), "host");
  assert.equal(log.asked.length + log.ran.length, 0);
});

test("a Deny refuses with -32001 and runs nothing; an unanswered card refuses too", async () => {
  const { exec, log } = setup({ answer: false });
  await assert.rejects(
    exec.run(COMPANION, REQUEST),
    (error) => error.code === -32001 && error.message === "The owner refused",
  );
  assert.equal(log.ran.length, 0);
  assert.equal(log.audit[0].decision, "denied");
  const silent = setup({ deps: { askOwner: async () => "timeout" } });
  await rejectsWith(silent.exec.run(COMPANION, REQUEST), "refused");
  assert.equal(silent.log.ran.length, 0);
  assert.equal(silent.log.audit[0].decision, "timeout");
});

test("the card holds the extension, host, exact argv, description and stdin hash, never the stdin", async () => {
  const { exec, log } = setup();
  const stdin = Buffer.from("STDIN-SECRET");
  await exec.run(COMPANION, {
    ...REQUEST,
    argv: ["sh", "-c", "echo SECRET\n"],
    stdin: stdin.toString("base64"),
  });
  assert.deepEqual(log.asked, [
    {
      extensionId: "ext.one",
      extensionName: "Extension one",
      hostId: "h-1",
      hostName: "Devbox",
      hostAddress: "user@devbox",
      argv: ["sh", "-c", "echo SECRET\n"],
      title: "Install the agent",
      stdinBytes: stdin.length,
      stdinSha256: require("node:crypto")
        .createHash("sha256")
        .update(stdin)
        .digest("hex"),
    },
  ]);
  assert.ok(!JSON.stringify(log.asked).includes("STDIN-SECRET"));
  await exec.run(COMPANION, { ...REQUEST, host: "h-2" });
  assert.equal(log.asked[1].hostAddress, "192.0.2.7:2200");
  assert.equal(log.asked[1].stdinSha256, null);
});

test("every call needs its own card, however soon after an Allow", async () => {
  const { exec, log } = setup();
  await exec.run(COMPANION, REQUEST);
  await exec.run(COMPANION, REQUEST);
  await exec.run(COMPANION, { ...REQUEST, host: "h-2" });
  await exec.run({ ...COMPANION, id: "ext.two" }, REQUEST);
  assert.equal(log.asked.length, 4);
  assert.equal(log.ran.length, 4);
});

test("a companion has one card on screen and one behind it; more calls are busy", async () => {
  const answers = [];
  const { exec, log } = setup({
    deps: {
      askOwner: () => new Promise((resolve) => answers.push(resolve)),
    },
  });
  const first = exec.run(COMPANION, REQUEST);
  const second = exec.run(COMPANION, { ...REQUEST, host: "h-2" });
  await rejectsWith(exec.run(COMPANION, { ...REQUEST, host: "h-3" }), "busy");
  // Another companion is not held up by the first one's queue.
  const other = exec.run(
    { ...COMPANION, id: "ext.two" },
    { ...REQUEST, host: "h-4" },
  );
  answers.forEach((resolve) => resolve("allowed"));
  await Promise.all([first, second, other]);
  assert.equal(log.ran.length, 3);
});

test("forget() makes a pending answer void", async () => {
  let answer;
  const cancelled = [];
  const { exec, log } = setup({
    deps: {
      askOwner: () => new Promise((resolve) => (answer = resolve)),
      cancelAsks: (id) => cancelled.push(id),
    },
  });
  const call = exec.run(COMPANION, REQUEST);
  exec.forget("ext.one");
  answer("allowed");
  await rejectsWith(call, "refused");
  assert.deepEqual(cancelled, ["ext.one"]);
  assert.equal(log.ran.length, 0);
});

test("a host that changed while the card was open is not run", async () => {
  const profiles = PROFILES.map((profile) => ({ ...profile }));
  let answer;
  const { exec, log } = setup({
    deps: {
      getProfiles: () => profiles,
      askOwner: () => new Promise((resolve) => (answer = resolve)),
    },
  });
  const call = exec.run(COMPANION, REQUEST);
  profiles[0].host = "user@elsewhere";
  answer("allowed");
  await rejectsWith(call, "host");
  assert.equal(log.ran.length, 0);
});

test("an allowed call that then finds the host busy is audited", async () => {
  let release;
  const { exec, log } = setup({
    deps: {
      execOnHost: () => new Promise((resolve) => (release = resolve)),
    },
  });
  // Both pass the early check and both get a card; the second finds the host
  // taken once its owner has said yes.
  const first = exec.run(COMPANION, REQUEST);
  const second = exec.run({ ...COMPANION, id: "ext.two" }, REQUEST);
  await rejectsWith(second, "busy");
  release({ code: 0, stdout: "", stderr: "" });
  await first;
  assert.deepEqual(
    log.audit.map((line) => [line.extensionId, line.decision]),
    [
      ["ext.two", "busy"],
      ["ext.one", "allowed"],
    ],
  );
});

test("limits: stdin 11 MiB decoded, timeout 300 s, title, argv", async () => {
  const { exec, log } = setup();
  const bad = async (patch) =>
    rejectsWith(exec.run(COMPANION, { ...REQUEST, ...patch }), "params");
  await bad({ stdin: "AAAA".repeat(3844779) }); // 11 MiB + 1 decoded
  await bad({ stdin: "not base64!" });
  await bad({ timeoutMs: 300001 });
  await bad({ timeoutMs: 0 });
  await bad({ title: "" });
  await bad({ title: "t".repeat(121) });
  await bad({ title: "two\nlines" });
  await bad({ title: "zero\u200bwidth" });
  await bad({ title: "bidi\u202eflip" });
  await bad({ title: "sep\u2028line" });
  await bad({ argv: ["x".repeat(70000)] });
  await bad({ argv: [] });
  await bad({ argv: "uname" });
  await bad({ argv: ["a\0b"] });
  assert.equal(log.asked.length + log.ran.length, 0);
  // The edge cases pass: exactly 11 MiB, 300 s, and a default of 120 s.
  const edge = Buffer.alloc(11 * 1024 * 1024, 1).toString("base64");
  await exec.run(COMPANION, { ...REQUEST, stdin: edge, timeoutMs: 300000 });
  assert.equal(log.ran[0].opts.input.length, 11 * 1024 * 1024);
  assert.equal(log.ran[0].opts.timeout, 300000);
  await exec.run(COMPANION, REQUEST);
  assert.equal(log.ran[1].opts.timeout, 120000);
});

test("at most one command per host and four in total; extra calls are busy", async () => {
  const gates = [];
  const { exec } = setup({
    deps: {
      execOnHost: () => new Promise((resolve) => gates.push(resolve)),
    },
  });
  const done = { code: 0, stdout: "", stderr: "" };
  const first = exec.run(COMPANION, REQUEST);
  await new Promise((resolve) => setImmediate(resolve));
  await rejectsWith(exec.run(COMPANION, REQUEST), "busy");
  const more = ["h-2", "h-3", "h-4"].map((host) =>
    exec.run({ ...COMPANION, id: `ext.${host}` }, { ...REQUEST, host }),
  );
  await new Promise((resolve) => setImmediate(resolve));
  await rejectsWith(
    exec.run({ ...COMPANION, id: "ext.h-5" }, { ...REQUEST, host: "h-5" }),
    "busy",
  );
  gates.forEach((release) => release(done));
  await Promise.all([first, ...more]);
  // The slots are free again.
  const again = exec.run(COMPANION, REQUEST);
  await new Promise((resolve) => setImmediate(resolve));
  gates.at(-1)(done);
  await again;
});

test("stdout and stderr keep the last 64 KiB", async () => {
  const big = "x".repeat(200000) + "THE-END";
  const { exec } = setup({
    result: { code: 3, stdout: big, stderr: "e".repeat(70000) + "!" },
  });
  const result = await exec.run(COMPANION, REQUEST);
  assert.equal(result.code, 3);
  assert.equal(Buffer.byteLength(result.stdout), 65536);
  assert.ok(result.stdout.endsWith("THE-END"));
  assert.equal(Buffer.byteLength(result.stderr), 65536);
  assert.ok(result.stderr.endsWith("!"));
  const killed = setup({
    result: { code: null, stdout: "", stderr: "timeout" },
  });
  assert.equal((await killed.exec.run(COMPANION, REQUEST)).code, null);
});

test("the audit line has hashes, decision, code and time, and never argv, stdin or output", async () => {
  const { exec, log } = setup({
    result: { code: 0, stdout: "OUTPUT-SECRET", stderr: "STDERR-SECRET" },
  });
  const stdin = Buffer.from("STDIN-SECRET");
  await exec.run(COMPANION, {
    ...REQUEST,
    argv: ["sh", "-c", "ARGV-SECRET"],
    stdin: stdin.toString("base64"),
  });
  assert.equal(log.audit.length, 1);
  const line = log.audit[0];
  const sha = (data) =>
    require("node:crypto").createHash("sha256").update(data).digest("hex");
  assert.deepEqual(
    Object.keys(line).sort(),
    [
      "argvSha256",
      "at",
      "code",
      "decision",
      "extensionId",
      "hostId",
      "stdinSha256",
    ].sort(),
  );
  assert.equal(
    line.argvSha256,
    sha(JSON.stringify(["sh", "-c", "ARGV-SECRET"])),
  );
  assert.equal(line.stdinSha256, sha(stdin));
  assert.equal(line.decision, "allowed");
  assert.equal(line.code, 0);
  const text = JSON.stringify(log.audit);
  for (const secret of [
    "ARGV-SECRET",
    "STDIN-SECRET",
    "OUTPUT-SECRET",
    "STDERR-SECRET",
    "Install the agent",
  ])
    assert.ok(!text.includes(secret), secret);
});

test("a failed run reports -32000 with the message and releases the host", async () => {
  let fail = true;
  const { exec } = setup({
    deps: {
      execOnHost: async () => {
        if (fail) throw new Error("ssh: connect refused");
        return { code: 0, stdout: "", stderr: "" };
      },
    },
  });
  await assert.rejects(
    exec.run(COMPANION, REQUEST),
    (error) => error.code === -32000 && /connect refused/.test(error.message),
  );
  fail = false;
  await exec.run(COMPANION, REQUEST);
});

test("list values: invalid rows are dropped, the rest kept, caps hold", () => {
  const view = { fields: [{ id: "hosts", type: "list" }] };
  const rows = checkResult(
    {
      values: {
        hosts: [
          {
            id: "h1",
            label: "ra2",
            detail: "user@devbox:22",
            tone: "ok",
            status: "In Remote",
            action: "Add",
          },
          { id: "h1", label: "dup" },
          { id: "x".repeat(101), label: "long id" },
          { id: "h2", label: "l".repeat(201) },
          { id: "h3", label: "bell\u0007" },
          { id: "h4", label: "ok", tone: "pink" },
          { id: "h5", label: "ok", status: 5 },
          { label: "no id" },
          "text",
          { id: "h6", label: "fine" },
        ],
      },
    },
    view,
  ).values.hosts;
  assert.deepEqual(
    rows.map((row) => row.id),
    ["h1", "h4", "h6"],
  );
  assert.equal(rows[1].tone, "neutral");
  const many = checkResult(
    {
      values: {
        hosts: Array.from({ length: 60 }, (_, i) => ({
          id: `r${i}`,
          label: "L",
        })),
      },
    },
    view,
  ).values.hosts;
  assert.equal(many.length, 50);
  assert.throws(
    () => checkResult({ values: { hosts: "nope" } }, view),
    /bad value/,
  );
});

// ---- the supervisor end to end, with the synthetic companion process ----

const FIXTURES = path.join(__dirname, "fixtures/extensions/companion-probe");
const MANIFEST = JSON.parse(
  fs.readFileSync(path.join(FIXTURES, "manifest.json"), "utf8"),
);
const ID = MANIFEST.id;
const SURFACE = "probe.companion-settings";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitFor(check, what, ms = 8000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(15);
  }
}

async function rig(t, { permissions } = {}) {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-hostexec-"));
  const home = path.join(base, "home");
  const localDir = path.join(base, "extensions");
  fs.mkdirSync(path.join(localDir, "probe"), { recursive: true });
  fs.mkdirSync(path.join(home, "bin"), { recursive: true });
  const manifest = JSON.parse(JSON.stringify(MANIFEST));
  if (permissions) manifest.companion.permissions = permissions;
  fs.writeFileSync(
    path.join(localDir, "probe", "manifest.json"),
    JSON.stringify(manifest),
  );
  fs.writeFileSync(
    path.join(home, "bin", "probe-companion"),
    `#!/bin/sh\nexec "${process.execPath}" "${path.join(FIXTURES, "companion.cjs")}" "$@"\n`,
    { mode: 0o755 },
  );
  const probe = (name) => path.join(home, "probe", name);
  const lines = (name) =>
    fs.existsSync(probe(name))
      ? fs
          .readFileSync(probe(name), "utf8")
          .split("\n")
          .filter(Boolean)
          .map((l) => JSON.parse(l))
      : [];
  const control = (name, value) => {
    fs.mkdirSync(path.dirname(probe(name)), { recursive: true });
    fs.writeFileSync(probe(name), JSON.stringify(value));
  };
  const pidsOf = () =>
    fs.existsSync(probe("starts"))
      ? fs
          .readFileSync(probe("starts"), "utf8")
          .split("\n")
          .filter(Boolean)
          .map(Number)
      : [];
  let profiles = PROFILES.slice(0, 2);
  const listeners = new Set();
  const asked = [];
  const ran = [];
  const audit = [];
  const companions = createCompanions({
    home,
    getHosts: () => profiles,
    execOnHost: async (endpoint, argv, opts) => {
      ran.push({ endpoint, argv, opts });
      return { code: 0, stdout: "from-host", stderr: "" };
    },
    subscribeHosts: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    audit: (line) => audit.push(line),
    env: {
      PATH: "",
      HOME: base,
      USER: "tester",
      LANG: "C",
      TMPDIR: os.tmpdir(),
      SUSHIAI_HOME: home,
    },
    backoffMs: [20, 20, 20],
    killGraceMs: 300,
    helloTimeoutMs: 20000,
  });
  companions.setAskOwner(async (question) => {
    asked.push(question);
    return "allowed";
  });
  const manager = new ExtensionManager({
    dataDir: path.join(base, "data"),
    localDir,
    companions,
  });
  t.after(async () => {
    await companions.stopAll();
    for (const pid of pidsOf())
      try {
        process.kill(pid, "SIGKILL");
      } catch {}
    fs.rmSync(base, { recursive: true, force: true });
  });
  await manager.ready;
  await companions.start();
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  await waitFor(
    async () => companions.status(ID)?.state === "running",
    "running",
  );
  return {
    manager,
    companions,
    lines,
    pids: pidsOf,
    control,
    asked,
    ran,
    audit,
    setProfiles(next) {
      profiles = next;
      for (const listener of listeners) listener(next);
    },
  };
}

test("hosts.changed goes to a companion with hosts.read on start and on every change", async (t) => {
  const r = await rig(t);
  const notes = () =>
    r.lines("notes").filter((n) => n.method === "hosts.changed");
  await waitFor(() => notes().length === 1, "the first list");
  assert.deepEqual(notes()[0].params.hosts, [
    { id: "h-1", name: "Devbox", host: "user@devbox" },
    { id: "h-2", name: "Edge", host: "192.0.2.7", port: 2200 },
  ]);
  r.setProfiles([...PROFILES.slice(0, 3), PROFILES[5]]);
  await waitFor(() => notes().length === 2, "the changed list");
  assert.deepEqual(
    notes()[1].params.hosts.map((h) => h.id),
    ["h-1", "h-2", "h-3"],
  );
  // An unchanged list is not sent again.
  r.setProfiles([...PROFILES.slice(0, 3), PROFILES[5]]);
  await sleep(150);
  assert.equal(notes().length, 2);
});

test("a list row button reaches the companion, which runs host.exec through the app", async (t) => {
  const r = await rig(t);
  r.control("list-rows", [
    {
      id: "h-1",
      label: "Devbox",
      status: "Not in Remote",
      tone: "warning",
      action: "Add to Remote",
    },
    { id: "", label: "dropped" },
  ]);
  const read = await r.manager.companionRead(ID, SURFACE);
  assert.deepEqual(read.values.hosts, [
    {
      id: "h-1",
      label: "Devbox",
      status: "Not in Remote",
      tone: "warning",
      action: "Add to Remote",
    },
  ]);
  r.control("exec-request", {
    host: "h-1",
    title: "Install the link",
    argv: ["sh", "-c", "uname -m"],
    stdin: Buffer.from("invite-secret").toString("base64"),
  });
  const result = await r.manager.companionRow(ID, SURFACE, "hosts", "h-1");
  assert.equal(result.message, "added");
  const call = r.lines("calls").find((c) => c.method === "host.add");
  assert.equal(call.params.row, "h-1");
  assert.equal(call.params.surfaceId, SURFACE);
  assert.deepEqual(r.lines("exec-responses")[0].result, {
    code: 0,
    stdout: "from-host",
    stderr: "",
  });
  assert.equal(r.asked.length, 1);
  assert.equal(r.asked[0].extensionName, "Companion probe");
  assert.equal(r.asked[0].hostName, "Devbox");
  assert.deepEqual(r.ran[0].argv, ["sh", "-c", "uname -m"]);
  assert.equal(r.ran[0].opts.input.toString(), "invite-secret");
  // A second call asks again.
  await r.manager.companionRow(ID, SURFACE, "hosts", "h-1");
  assert.equal(r.asked.length, 2);
  assert.deepEqual(r.asked[0].argv, ["sh", "-c", "uname -m"]);
  assert.equal(r.audit.length, 2);
  assert.ok(!JSON.stringify(r.audit).includes("invite-secret"));
});

test("a card answered after the companion restarted is void", async (t) => {
  const r = await rig(t);
  let answer;
  r.companions.setAskOwner(() => new Promise((resolve) => (answer = resolve)));
  r.control("exec-request", { host: "h-1", title: "Late", argv: ["true"] });
  const pending = r.manager
    .companionRow(ID, SURFACE, "hosts", "h-1")
    .catch(() => {});
  await waitFor(() => answer, "the card");
  const [first] = r.pids();
  process.kill(first, "SIGKILL");
  await waitFor(async () => r.pids().length === 2, "a restart");
  answer(true);
  await pending;
  assert.equal(r.ran.length, 0);
});

test("only the field and row ids come from the renderer; an unknown or method-less list is refused", async (t) => {
  const r = await rig(t);
  await assert.rejects(
    () => r.manager.companionRow(ID, SURFACE, "note", "x"),
    /Unknown companion list/,
  );
  await assert.rejects(
    () => r.manager.companionRow(ID, SURFACE, "missing", "x"),
    /Unknown companion list/,
  );
});

test("a companion without hosts.exec gets a permission error from host.exec", async (t) => {
  const r = await rig(t, { permissions: ["hosts.read"] });
  r.control("exec-request", { host: "h-1", title: "No", argv: ["true"] });
  await r.manager.companionRow(ID, SURFACE, "hosts", "h-1");
  assert.equal(r.lines("exec-responses")[0].error.code, -32003);
  assert.equal(r.asked.length + r.ran.length, 0);
});
