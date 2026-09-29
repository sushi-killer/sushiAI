// A stand-in for the orchd binary: serves ping / task.list / subscribe /
// shutdown on its unix socket and writes the same control.token and orchd.pid
// files the real daemon does. Started as `fake-orchd serve --data D --socket S`.
const net = require("node:net");
const fs = require("node:fs");
const path = require("node:path");

const args = process.argv.slice(2);
const flag = (name) => args[args.indexOf(name) + 1];
const data = flag("--data");
const socketPath = flag("--socket");
fs.mkdirSync(data, { recursive: true });
const token = `token-${process.pid}`;
fs.writeFileSync(path.join(data, "control.token"), token, { mode: 0o600 });
fs.writeFileSync(path.join(data, "orchd.pid"), String(process.pid));

const tasks = [
  {
    id: "remote-task-1",
    title: "Remote job",
    status: "waiting",
    repo: "/srv/app",
    question: { text: "Which database?", options: ["Postgres", "SQLite"] },
    attempts: [],
    decisions: [],
    updatedAt: 1,
  },
];
const subscribers = new Set();
const server = net.createServer((socket) => {
  let buffer = "";
  socket.on("error", () => {});
  socket.on("close", () => subscribers.delete(socket));
  socket.on("data", (chunk) => {
    buffer += chunk;
    let boundary;
    while ((boundary = buffer.indexOf("\n")) >= 0) {
      const message = JSON.parse(buffer.slice(0, boundary));
      buffer = buffer.slice(boundary + 1);
      const reply = (result, error) =>
        socket.write(
          JSON.stringify(
            error
              ? { id: message.id, error: { message: error } }
              : { id: message.id, result },
          ) + "\n",
        );
      if (message.method !== "ping" && message.auth !== token)
        return reply(null, "unauthorized");
      if (message.method === "ping")
        reply({ version: "fake", pid: process.pid, dataDir: data });
      else if (message.method === "task.list") reply(tasks);
      else if (message.method === "task.get") reply(tasks[0]);
      else if (message.method === "subscribe") {
        subscribers.add(socket);
        socket.write(JSON.stringify({ event: "task", task: tasks[0] }) + "\n");
      } else if (message.method === "shutdown") {
        reply({});
        setTimeout(() => process.exit(0), 20);
      } else reply({});
    }
  });
});
fs.rmSync(socketPath, { force: true });
server.listen(socketPath);
