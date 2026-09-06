const net = require("node:net");
const { randomUUID } = require("node:crypto");

function request(socketPath, method, params = {}, timeout = 5000) {
  return new Promise((resolve, reject) => {
    const id = randomUUID();
    const socket = net.createConnection(socketPath);
    let buffer = "",
      settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      error ? reject(error) : resolve(value);
    };
    socket.setEncoding("utf8");
    socket.setTimeout(timeout, () =>
      finish(new Error("Herdr did not respond. Check the socket in Settings.")),
    );
    socket.on("error", (error) => finish(error));
    socket.on("close", () => {
      if (!settled) finish(new Error("Herdr disconnected before responding."));
    });
    socket.on("connect", () =>
      socket.write(JSON.stringify({ id, method, params }) + "\n"),
    );
    socket.on("data", (chunk) => {
      buffer += chunk;
      if (buffer.length > 16 * 1024 * 1024)
        return finish(new Error("Herdr response exceeds 16 MB."));
      let boundary;
      while ((boundary = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 1);
        if (!line.trim()) continue;
        let message;
        try {
          message = JSON.parse(line);
        } catch {
          return finish(new Error("Invalid JSON from Herdr."));
        }
        if (message.id !== id) continue;
        if (message.error) return finish(new Error(message.error.message));
        finish(null, message.result);
      }
    });
  });
}

// xterm emits terminal sequences; Herdr's API expects named keys for controls.
function inputCommands(data) {
  const sequences = {
    "\x1b[A": "Up",
    "\x1b[B": "Down",
    "\x1b[C": "Right",
    "\x1b[D": "Left",
    "\x1b[H": "Home",
    "\x1b[F": "End",
    "\x1b[3~": "Delete",
    "\x1b[5~": "PageUp",
    "\x1b[6~": "PageDown",
    "\x1b[Z": "Shift+Tab",
    "\r": "Enter",
    "\n": "Enter",
    "\t": "Tab",
    "\x7f": "Backspace",
    "\x1b": "Escape",
  };
  const commands = [];
  let text = "";
  const flush = () => {
    if (text) {
      commands.push({ text });
      text = "";
    }
  };
  // Bracketed paste must stay literal, including newlines.
  if (data.startsWith("\x1b[200~") && data.endsWith("\x1b[201~"))
    return [{ text: data.slice(6, -6) }];
  for (let i = 0; i < data.length;) {
    const sequence = Object.keys(sequences).find((key) =>
      data.startsWith(key, i),
    );
    if (sequence) {
      flush();
      commands.push({ keys: [sequences[sequence]] });
      i += sequence.length;
    } else if (data.charCodeAt(i) > 0 && data.charCodeAt(i) < 27) {
      flush();
      commands.push({
        keys: ["Ctrl+" + String.fromCharCode(96 + data.charCodeAt(i))],
      });
      i++;
    } else text += data[i++];
  }
  flush();
  return commands;
}
module.exports = { request, inputCommands };
