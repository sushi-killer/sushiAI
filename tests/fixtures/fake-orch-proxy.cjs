// A fake `ssh host sushiai proxy`: it speaks the daemon protocol on stdio like
// the real proxy and serves a few `orch.*` methods from memory. FAKE_PROXY_DIR
// holds mode.json (read on every spawn), args.log (the ssh arguments of every
// spawn) and requests.log (one JSON line per orch request, so a test sees what
// reached the host).
//   {capabilities}   the hello capabilities (default sessions, attach, orch)
//   {tasks}          tasks `orch.task.list` starts with
//   {exit, stderr}   print stderr and exit with the code (no protocol)
const fs = require("node:fs");
const path = require("node:path");
const { createDecoder, encode } = require("../../electron/daemon/frame.cjs");

const dir = process.env.FAKE_PROXY_DIR;
const mode = JSON.parse(fs.readFileSync(path.join(dir, "mode.json"), "utf8"));

// `ssh -G host`: print the resolved config; not a request.
if (process.argv.includes("-G")) {
  process.stdout.write(
    "user dev\nhostname devbox.example.test\nport 22\n",
    () => process.exit(0),
  );
  return;
}

fs.appendFileSync(
  path.join(dir, "args.log"),
  JSON.stringify(process.argv.slice(2)) + "\n",
);

if (mode.exit !== undefined) {
  process.stderr.write(mode.stderr || "", () => process.exit(mode.exit));
} else {
  const decoder = createDecoder();
  const send = (message) =>
    process.stdout.write(
      encode({
        kind: "J",
        json: JSON.stringify({ jsonrpc: "2.0", ...message }),
      }),
    );
  const tasks = [...(mode.tasks ?? [])];
  const handlers = {
    "orch.settings.get": () => ({
      routes: [],
      chatTools: [],
      tiers: { light: "", standard: "", hard: "" },
      review: "",
      sandbox: "native",
      allowedDomains: [],
      protectedPaths: [],
      maxAttempts: 3,
      parallel: 2,
      childParallel: 2,
      orchestrator: "",
    }),
    "orch.secrets.set": () => ({}),
    "orch.task.list": () => tasks,
    "orch.costs.summary": () => ({
      rows: [],
      totals: {
        costUsd: 0,
        runs: 0,
        tokens: { input: 0, cached: 0, output: 0 },
        cacheHitRate: 0,
      },
    }),
    "orch.chat.get": (params) => ({
      repo: params.repo,
      id: "chat-1",
      createdAt: 1,
      messages: [],
      busy: false,
    }),
    "orch.task.create": (params, notify) => {
      const task = {
        id: `task-${tasks.length + 1}`,
        title: params.title,
        repo: params.repo,
        status: "queued",
        updatedAt: Date.now(),
        attempts: [],
        decisions: [],
        archived: false,
      };
      tasks.push(task);
      // The module announces the new task, then it starts running.
      setTimeout(
        () => notify("orch.event", { event: "task", task: { ...task } }),
        20,
      );
      return task;
    },
  };
  process.stdin.on("data", (chunk) => {
    for (const frame of decoder.push(chunk)) {
      const message = JSON.parse(frame.json);
      if (message.method === "hello")
        return send({
          id: message.id,
          result: {
            protocol: 1,
            capabilities: mode.capabilities ?? ["sessions", "attach", "orch"],
            daemon: "1.0.0",
            host: "devbox",
          },
        });
      if (message.method?.startsWith("orch."))
        fs.appendFileSync(
          path.join(dir, "requests.log"),
          JSON.stringify({ method: message.method, params: message.params }) +
            "\n",
        );
      const handler = handlers[message.method];
      // Anything else answers an empty list or object, which the panel reads.
      const fallback = /\.(list|tools|toolServers|timeline|catalogue)$/.test(
        message.method ?? "",
      )
        ? []
        : {};
      const result = handler
        ? handler(message.params ?? {}, (method, params) =>
            send({ method, params }),
          )
        : message.method === "session.list"
          ? []
          : fallback;
      if (message.id !== undefined) send({ id: message.id, result });
    }
  });
  process.stdin.on("end", () => process.exit(0));
}
