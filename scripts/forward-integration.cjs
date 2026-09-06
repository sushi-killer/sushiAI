const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const { spawn } = require("node:child_process");
const { once } = require("node:events");
const { Connections, quote, run } = require("../electron/connections.cjs");
(async () => {
  const host = process.env.SUSHIAI_SSH_HOST;
  if (!host) throw new Error("Set SUSHIAI_SSH_HOST to your SSH test host.");
  const dir = await fs.mkdtemp("/tmp/sushiai-forward-test-");
  const c = new Connections(dir);
  await c.init();
  let server;
  try {
    const p = await c.save({
      host,
      socket:
        process.env.SUSHIAI_SSH_SOCKET ||
        "~/.config/herdr/sessions/sushiai/herdr.sock",
    });
    const code = `from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import threading,sys
class Handler(BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200);self.end_headers();self.wfile.write(b'PORT_FORWARD_OK')
 def log_message(self,*args): pass
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
print(server.server_port,flush=True)
sys.stdin.read()
server.shutdown()`;
    server = spawn(
      "/usr/bin/ssh",
      [...c.args(p), p.host, "python3 -u -c " + quote(code)],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    const port = await new Promise((resolve, reject) => {
      let output = "";
      const timeout = setTimeout(
        () => reject(Error("Remote server timeout")),
        10000,
      );
      server.stdout.on("data", (d) => {
        output += d;
        if (output.includes("\n")) {
          clearTimeout(timeout);
          resolve(Number(output.trim()));
        }
      });
      server.on("error", reject);
      server.on("exit", () => {
        clearTimeout(timeout);
        reject(Error("Remote server exited"));
      });
    });
    const local = await c.forward("ssh:" + p.id, port);
    assert.equal(
      await (await fetch("http://127.0.0.1:" + local)).text(),
      "PORT_FORWARD_OK",
    );
    await c.disconnect("ssh:" + p.id);
    await assert.rejects(fetch("http://127.0.0.1:" + local));
    await run("/usr/bin/ssh", [...c.args(p), p.host, "true"]);
    console.log(
      "PASS: remote localhost forwarding, owned-tunnel cleanup, shared SSH session remains usable",
    );
  } finally {
    if (server) {
      const closed = once(server, "close");
      server.stdin.end();
      const timeout = setTimeout(() => server.kill(), 3000);
      await closed;
      clearTimeout(timeout);
    }
    await c.close();
    await fs.rm(dir, { recursive: true, force: true });
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
