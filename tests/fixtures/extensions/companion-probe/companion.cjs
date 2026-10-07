// A synthetic companion process for the suite. It speaks the daemon frame
// protocol on stdio. Its behaviour is steered by files under
// $SUSHIAI_HOME/probe (the only environment a companion inherits):
//   exit-now      write "boom" to stderr and exit 1 before answering hello
//   ignore-term   ignore SIGTERM, so only SIGKILL ends it
//   no-hello      never answer hello (the app gives up on it)
//   list-rows     JSON: the value view.read gives a field named "hosts"
//   exec-request  JSON: the host.exec request host.add sends to the app; the
//                 app's answer is written to exec-response
// It records what it was started with and what it was asked.
const fs = require("node:fs");
const path = require("node:path");
const {
  createDecoder,
  encode,
} = require("../../../../electron/daemon/frame.cjs");

const dir = path.join(process.env.SUSHIAI_HOME, "probe");
fs.mkdirSync(dir, { recursive: true });
const note = (name, text) => fs.appendFileSync(path.join(dir, name), text);
note("starts", `${process.pid}\n`);
note(
  "started-with",
  JSON.stringify({
    args: process.argv.slice(2),
    env: Object.keys(process.env).sort(),
  }) + "\n",
);
if (fs.existsSync(path.join(dir, "exit-now"))) {
  process.stderr.write("boom: probe companion refused to start\n");
  process.exit(1);
}
if (fs.existsSync(path.join(dir, "ignore-term")))
  process.on("SIGTERM", () => {});

const send = (message) =>
  process.stdout.write(
    encode({ kind: "J", json: JSON.stringify({ jsonrpc: "2.0", ...message }) }),
  );
const decoder = createDecoder();
let nextRequest = 1000;
const waiting = new Map();
const readControl = (name) => {
  const file = path.join(dir, name);
  return fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : null;
};
process.stdin.on("data", (chunk) => {
  for (const frame of decoder.push(chunk)) {
    const { id, method, params, result, error } = JSON.parse(frame.json);
    if (!method) {
      waiting.get(id)?.({ result, error });
      continue;
    }
    if (id === undefined) {
      note("notes", JSON.stringify({ method, params }) + "\n");
      continue;
    }
    if (method === "hello") {
      if (!fs.existsSync(path.join(dir, "no-hello")))
        send({ id, result: { protocol: 1 } });
    } else if (method === "view.read")
      send({
        id,
        result: {
          values: {
            service: { text: "Connected", tone: "ok" },
            pairing: "probe-pairing-code",
            note: null,
            ...(readControl("list-rows")
              ? { hosts: readControl("list-rows") }
              : {}),
          },
        },
      });
    else if (method === "host.add") {
      note("calls", JSON.stringify({ method, params }) + "\n");
      const requestId = nextRequest++;
      const answer = new Promise((resolve) => waiting.set(requestId, resolve));
      send({
        id: requestId,
        method: "host.exec",
        params: readControl("exec-request"),
      });
      answer.then((reply) => {
        note("exec-responses", JSON.stringify(reply) + "\n");
        send({ id, result: { message: "added" } });
      });
    } else if (method === "hosts.import") {
      note("calls", JSON.stringify({ method, params }) + "\n");
      send({
        id,
        result: {
          message: "imported",
          values: { note: `${(params.hosts || []).length} hosts` },
        },
      });
    } else if (method === "device.add") {
      note("calls", JSON.stringify({ method, params }) + "\n");
      send({ id, result: { values: { pairing: "probe-new-code" } } });
      send({ method: "view.changed", params: { surfaceId: params.surfaceId } });
    } else send({ id, error: { code: -32601, message: "unknown method" } });
  }
});
process.stdin.on("end", () => process.exit(0));
