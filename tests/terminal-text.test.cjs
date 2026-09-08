const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const {
  terminalEnvironment,
  createFrameDecoder,
} = require("../electron/terminal-text.cjs");

test("UTF-8 Cyrillic and Nerd Font symbols survive every frame boundary", () => {
  const text = "Привет русский текст   󰂺 README.md";
  const bytes = Buffer.from(text);
  for (let split = 1; split < bytes.length; split++) {
    const decode = createFrameDecoder();
    assert.equal(
      decode(bytes.subarray(0, split).toString("base64")) +
        decode(bytes.subarray(split).toString("base64")),
      text,
    );
  }
  const decode = createFrameDecoder();
  assert.equal(
    [...bytes]
      .map((byte) => decode(Buffer.from([byte]).toString("base64")))
      .join(""),
    text,
  );
});

test("full screen snapshot discards a partial character from the replaced screen", () => {
  const decode = createFrameDecoder();
  assert.equal(decode(Buffer.from([0xd0]).toString("base64")), "");
  assert.equal(
    decode(Buffer.from("Новый экран").toString("base64"), true),
    "Новый экран",
  );
});

test("GUI and ASCII locale launches enable multibyte zsh editing", () => {
  for (const env of [
    {},
    { LANG: "C", LC_ALL: "C" },
    { LANG: "en_US.UTF-8", LC_CTYPE: "C" },
  ]) {
    const result = spawnSync(
      "/bin/zsh",
      ["-f", "-c", "text=Привет; print -r -- ${#text}"],
      { env: terminalEnvironment(env), encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "6");
  }
  const env = { LANG: "ru_RU.UTF-8", LC_ALL: "ru_RU.UTF-8" };
  assert.equal(terminalEnvironment(env).LC_ALL, env.LC_ALL);
  assert.deepEqual(env, { LANG: "ru_RU.UTF-8", LC_ALL: "ru_RU.UTF-8" });
});

test("Herdr creation RPCs carry UTF-8 without relying on the Electron environment", () => {
  const { herdrLaunchParams } = require("../electron/terminal-text.cjs");
  for (const method of ["workspace.create", "pane.split"]) {
    const params = { cwd: "/tmp", env: { PROJECT_FLAG: "kept", LC_ALL: "C" } };
    const fixed = herdrLaunchParams(method, params);
    assert.equal(fixed.env.LANG, "en_US.UTF-8");
    assert.equal(fixed.env.LC_CTYPE, "en_US.UTF-8");
    assert.equal(fixed.env.LC_ALL, "en_US.UTF-8");
    assert.equal(fixed.env.PROJECT_FLAG, "kept");
    assert.equal(params.env.LC_ALL, "C");
  }
  const input = { pane_id: "pane", text: "Привет" };
  assert.equal(herdrLaunchParams("pane.send_input", input), input);
});
