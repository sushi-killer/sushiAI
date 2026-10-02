const http = require("node:http");
const path = require("node:path");
const fs = require("node:fs");
const { randomBytes } = require("node:crypto");
const ANNOTATE = fs.readFileSync(
  path.join(__dirname, "preview-annotate.js"),
  "utf8",
);
/** Puts the comment script at the end of an HTML page (before `</body>` when
 * there is one), so the Preview pane can take comments on any page. */
function decodeUtf8(buffer) {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(buffer);
  } catch {
    return null;
  }
}
function withAnnotations(html) {
  const tag = `<script>${ANNOTATE}</script>`;
  const end = html.toLowerCase().lastIndexOf("</body>");
  return end < 0 ? html + tag : html.slice(0, end) + tag + html.slice(end);
}
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
  grant(endpoint, root, file, { annotate = false } = {}) {
    const token = randomBytes(24).toString("hex");
    this.grants.set(token, { endpoint, root, annotate: annotate === true });
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
      const raw = Buffer.from(data.base64, "base64");
      const page = grant.annotate && /\.html?$/i.test(relative);
      const text = page ? decodeUtf8(raw) : null;
      // A page that is not valid UTF-8 is served as it is: re-encoding it
      // would corrupt it.
      const body =
        text === null ? raw : Buffer.from(withAnnotations(text), "utf8");
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
        // The page may be opened in the system browser: it cannot read its
        // sibling files back with fetch, XHR or a WebSocket.
        ...(page ? { "Content-Security-Policy": "connect-src 'none'" } : {}),
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
module.exports = { PreviewServer, withAnnotations };
