const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { once } = require("node:events");
const { existsSync } = require("node:fs");
const { request } = require("../electron/herdr.cjs");

function streamTestBinary() {
  if (process.env.SUSHIAI_HERDR_BINARY) return process.env.SUSHIAI_HERDR_BINARY;
  const binary = [
    path.join(os.homedir(), ".local/bin/herdr"),
    "/usr/local/bin/herdr",
    ...(process.env.PATH || "")
      .split(path.delimiter)
      .map((directory) => path.join(directory, "herdr")),
  ].find(existsSync);
  if (!binary)
    throw new Error("Set SUSHIAI_HERDR_BINARY to a compatible Herdr CLI.");
  return binary;
}

async function startStreamDaemon(binary) {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-stream-daemon-"),
  );
  const socket = path.join(directory, "herdr.sock");
  const config = path.join(directory, "config.toml");
  const shell = process.platform === "darwin" ? "/bin/zsh" : "/bin/bash";
  await fs.writeFile(
    config,
    `[update]\nversion_check = false\nmanifest_check = false\n[terminal]\ndefault_shell = '${shell}'\n`,
  );
  const child = spawn(binary, ["server"], {
    env: {
      ...process.env,
      HOME: directory,
      XDG_CONFIG_HOME: directory,
      HERDR_CONFIG_PATH: config,
      HERDR_SOCKET_PATH: socket,
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let diagnostic = "";
  child.stderr.on("data", (data) => {
    diagnostic = (diagnostic + data).slice(-4000);
  });
  child.on("error", (error) => {
    diagnostic = error.message;
  });
  const close = async () => {
    if (child.exitCode === null && child.signalCode === null) {
      const exited = once(child, "close");
      child.kill("SIGTERM");
      await exited;
    }
    await fs.rm(directory, { recursive: true, force: true });
  };
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (child.exitCode !== null || child.signalCode !== null)
        throw new Error(`Isolated Herdr exited: ${diagnostic}`);
      try {
        await request(socket, "session.snapshot", {}, 200);
        return { binary, socket, directory, close };
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
    throw new Error(`Isolated Herdr did not start: ${diagnostic}`);
  } catch (error) {
    await close();
    throw error;
  }
}

module.exports = { startStreamDaemon, streamTestBinary };
