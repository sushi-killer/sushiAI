// A fake `ssh` for tests: it runs the "remote" command locally under a fake
// HOME with a controlled PATH, and turns `-N -L local:remote` into a socket
// proxy. `FAKE_SSH_CONFIG` names a JSON file {home, bin, log}.
const { spawn } = require("node:child_process");
const net = require("node:net");
const fs = require("node:fs");

const config = JSON.parse(fs.readFileSync(process.env.FAKE_SSH_CONFIG, "utf8"));
const argv = process.argv.slice(2);
fs.appendFileSync(config.log, JSON.stringify(argv) + "\n");

let mode = "exec";
let forward = null;
const rest = [];
for (let i = 0; i < argv.length; i++) {
  const arg = argv[i];
  if (arg === "-T" || arg === "-tt") continue;
  if (arg === "-N") mode = "forward";
  else if (arg === "-o" || arg === "-p") i++;
  else if (arg === "-G") mode = "G";
  else if (arg === "-O") {
    mode = "O";
    i++;
  } else if (arg === "-L") forward = argv[++i];
  else rest.push(arg);
}

if (mode === "G") {
  process.stdout.write(`hostname ${rest[0]}\n`);
} else if (mode === "O") {
  process.exit(255);
} else if (mode === "forward") {
  const [local, remote] = forward.split(":");
  try {
    fs.rmSync(local, { force: true });
  } catch {}
  const server = net.createServer((client) => {
    const upstream = net.createConnection(remote);
    client.pipe(upstream);
    upstream.pipe(client);
    client.on("error", () => upstream.destroy());
    upstream.on("error", () => client.destroy());
    client.on("close", () => upstream.destroy());
  });
  server.listen(local);
  process.on("SIGTERM", () => process.exit(0));
} else {
  const child = spawn("/bin/sh", ["-c", rest[1]], {
    stdio: "inherit",
    env: { HOME: config.home, PATH: config.bin, TMPDIR: config.home },
  });
  child.on("close", (code) => process.exit(code ?? 1));
}
