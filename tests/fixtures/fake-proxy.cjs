// A fake `ssh` / connector command that speaks the daemon protocol on stdio,
// like `sushiai proxy`. FAKE_PROXY_DIR holds mode.json, read on every spawn
// (so a test can change it live), and spawns.log, one line per spawn.
//   {exit, stderr}          print stderr, exit with the code (no protocol)
//   {dieAfterMs, dieCode}   serve, then exit (default code 2) after the delay
//   {noPing}                serve, but never answer `$/ping`
//   {protocol}              hello answers with this protocol number
//   {gPort}                 the port `ssh -G` reports (default 22)
const fs = require("node:fs");
const path = require("node:path");
const { createDecoder, encode } = require("../../electron/daemon/frame.cjs");

const dir = process.env.FAKE_PROXY_DIR;
// `ssh -G host`: print the resolved config, and do not count as a spawn.
if (process.argv.includes("-G")) {
  const mode = JSON.parse(fs.readFileSync(path.join(dir, "mode.json"), "utf8"));
  process.stdout.write(
    `user dev\nhostname devbox.example.test\nport ${mode.gPort ?? 22}\n`,
    () => process.exit(0),
  );
  return;
}
fs.appendFileSync(
  path.join(dir, "spawns.log"),
  JSON.stringify(process.argv.slice(2)) + "\n",
);
const mode = JSON.parse(fs.readFileSync(path.join(dir, "mode.json"), "utf8"));

if (mode.exit !== undefined) {
  process.stderr.write(mode.stderr || "", () => process.exit(mode.exit));
} else {
  const decoder = createDecoder();
  const reply = (id, result) =>
    process.stdout.write(
      encode({
        kind: "J",
        json: JSON.stringify({ jsonrpc: "2.0", id, result }),
      }),
    );
  process.stdin.on("data", (chunk) => {
    for (const frame of decoder.push(chunk)) {
      const message = JSON.parse(frame.json);
      if (message.method === "hello")
        reply(message.id, {
          protocol: mode.protocol ?? 1,
          capabilities: ["sessions", "attach"],
          daemon: "1.0.0",
          host: "devbox",
        });
      else if (message.method === "session.list") reply(message.id, []);
      else if (message.method === "$/ping" && mode.noPing) continue;
      else reply(message.id, {});
    }
  });
  process.stdin.on("end", () => process.exit(0));
  if (mode.dieAfterMs)
    setTimeout(() => process.exit(mode.dieCode ?? 2), mode.dieAfterMs);
}
