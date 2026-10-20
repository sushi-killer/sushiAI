const http = require("node:http");
const path = require("node:path");
const fs = require("node:fs");
const { randomBytes } = require("node:crypto");
const { once } = require("node:events");
// One read of a range stays well under the 16 MB limit of a single read.
const READ_CHUNK = 8 * 1024 * 1024;
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
  ".mov",
  ".txt",
  ".pdf",
  ".avif",
]);
/** A path relative to the root, subfolders allowed; never absolute and never
 * climbing out with `..`. */
function isRelativeFile(file) {
  if (typeof file !== "string" || !file || file.includes("\0")) return false;
  if (file.startsWith("/") || file.includes("\\") || /^[a-z]:/i.test(file))
    return false;
  return file.split("/").every((part) => part && part !== "." && part !== "..");
}
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
    if (!isRelativeFile(file))
      throw new Error("Preview file must be a path inside the project");
    const token = randomBytes(24).toString("hex");
    // The page may load what sits in its own folder and below, as before the
    // root became the project; the folders above it (a `.claude` worktree
    // path, say) are fixed by the grant, not requested.
    const dir = path.posix.dirname(file);
    this.grants.set(token, {
      endpoint,
      root,
      dir: dir === "." ? [] : dir.split("/"),
      annotate: annotate === true,
    });
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
      const below =
        grant && grant.dir.every((part, index) => parts[index] === part)
          ? parts.slice(grant.dir.length)
          : null;
      if (
        !grant ||
        !below ||
        below.some(
          (part) =>
            part.startsWith(".") || part.includes("\\") || part.includes("\0"),
        ) ||
        !allowed.has(path.extname(relative).toLowerCase())
      ) {
        res.writeHead(403);
        return res.end("Preview access denied");
      }
      const read = (extra) =>
        this.connections.inspect(grant.endpoint, {
          operation: "read",
          root: grant.root,
          path: relative,
          ...extra,
        });
      const page = grant.annotate && /\.html?$/i.test(relative);
      let info, slice;
      if (page) {
        // A page is small and may be rewritten by the comment script, so it
        // is read whole.
        info = await read({});
        const raw = Buffer.from(info.base64, "base64");
        const text = decodeUtf8(raw);
        // A page that is not valid UTF-8 is served as it is: re-encoding it
        // would corrupt it.
        const body =
          text === null ? raw : Buffer.from(withAnnotations(text), "utf8");
        info = { ...info, size: body.length };
        slice = async function* (from, to) {
          yield body.subarray(from, to + 1);
        };
      } else {
        // Everything else is read by byte range, so a seek moves one slice
        // and a large file never travels whole.
        info = await read({ offset: 0, length: 0 });
        slice = async function* (from, to) {
          for (let at = from; at <= to; at += READ_CHUNK) {
            const part = await read({
              offset: at,
              length: Math.min(READ_CHUNK, to - at + 1),
            });
            yield Buffer.from(part.base64, "base64");
          }
        };
      }
      const total = info.size;
      const etag = `"${info.version}"`;
      const mime =
        relative.endsWith(".js") || relative.endsWith(".mjs")
          ? "text/javascript"
          : info.mime;
      // A video player seeks with byte ranges; without a 206 it cannot. A
      // range made against another version of the file is ignored (If-Range).
      const ifRange = req.headers["if-range"];
      const range =
        ifRange !== undefined && ifRange !== etag
          ? null
          : /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || "");
      let start = 0,
        end = total - 1;
      if (range && (range[1] || range[2])) {
        start = range[1] ? Number(range[1]) : total - Number(range[2]);
        end = range[1] && range[2] ? Number(range[2]) : total - 1;
        end = Math.min(end, total - 1);
        start = Math.max(start, 0);
        if (start > end) {
          res.writeHead(416, { "Content-Range": `bytes */${total}` });
          return res.end();
        }
      }
      const partial = !!range && (start > 0 || end < total - 1);
      res.writeHead(partial ? 206 : 200, {
        ...(partial
          ? { "Content-Range": `bytes ${start}-${end}/${total}` }
          : {}),
        "Accept-Ranges": "bytes",
        ETag: etag,
        "Content-Type": mime,
        "Content-Length": end - start + 1,
        "X-Content-Type-Options": "nosniff",
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
        // The page may be opened in the system browser: a sandbox without
        // allow-same-origin gives it an opaque origin, so it cannot read
        // sibling files back (fetch, XHR, WebSocket) or script an iframe of
        // them. Its own scripts and the postMessage to the pane still run.
        ...(page
          ? {
              "Content-Security-Policy":
                "sandbox allow-scripts; connect-src 'none'; frame-src 'none'",
            }
          : {}),
      });
      if (req.method === "HEAD") return res.end();
      for await (const chunk of slice(start, end)) {
        if (res.destroyed) return;
        if (!res.write(chunk)) await once(res, "drain");
      }
      res.end();
    } catch {
      // Once the headers are out the status cannot change: cut the stream so
      // the player sees a failed load, not a short file.
      if (res.headersSent) return res.destroy();
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
module.exports = { PreviewServer, withAnnotations, isRelativeFile };
