const http = require("node:http");
const path = require("node:path");
const { randomBytes } = require("node:crypto");
const allowed = new Set([
  ".html",
  ".htm",
  ".css",
  ".js",
  ".mjs",
  ".json",
  ".png",
  ".jpg",
  ".jpeg",
  ".webp",
  ".gif",
  ".svg",
  ".ico",
  ".woff",
  ".woff2",
  ".ttf",
  ".mp4",
  ".webm",
  ".txt",
  ".pdf",
  ".avif",
]);
class PreviewServer {
  constructor(connections) {
    this.connections = connections;
    this.grants = new Map();
    this.server = http.createServer((req, res) => this.serve(req, res));
  }
  async start() {
    await new Promise((resolve) => this.server.listen(0, "127.0.0.1", resolve));
    this.port = this.server.address().port;
  }
  grant(endpoint, root, file) {
    const token = randomBytes(24).toString("hex");
    this.grants.set(token, { endpoint, root });
    return `http://127.0.0.1:${this.port}/${token}/${file.split("/").map(encodeURIComponent).join("/")}`;
  }
  async serve(req, res) {
    try {
      if (!["GET", "HEAD"].includes(req.method)) {
        res.writeHead(405);
        return res.end();
      }
      const url = new URL(req.url, "http://127.0.0.1");
      const parts = url.pathname.split("/").slice(1).map(decodeURIComponent),
        token = parts.shift();
      const grant = this.grants.get(token),
        relative = parts.join("/");
      if (
        !grant ||
        parts.some(
          (part) =>
            part.startsWith(".") || part.includes("\\") || part.includes("\0"),
        ) ||
        !allowed.has(path.extname(relative).toLowerCase())
      ) {
        res.writeHead(403);
        return res.end("Preview access denied");
      }
      const data = await this.connections.inspect(grant.endpoint, {
        operation: "read",
        root: grant.root,
        path: relative,
      });
      const body = Buffer.from(data.base64, "base64");
      const mime =
        relative.endsWith(".js") || relative.endsWith(".mjs")
          ? "text/javascript"
          : data.mime;
      res.writeHead(200, {
        "Content-Type": mime,
        "Content-Length": body.length,
        "X-Content-Type-Options": "nosniff",
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
      });
      res.end(req.method === "HEAD" ? undefined : body);
    } catch {
      res.writeHead(404);
      res.end("File not found or unavailable");
    }
  }
  close() {
    this.grants.clear();
    this.server.closeAllConnections();
    this.server.close();
  }
}
module.exports = { PreviewServer };
