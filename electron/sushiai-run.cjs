const { execFile } = require("node:child_process");

/** Runs this machine's `sushiai` binary with `args`, `input` on stdin;
 * resolves its stdout, rejects with its stderr when it fails. */
function runSushiai(
  binary,
  args,
  { input = "", env = process.env, timeout = 30000 } = {},
) {
  return new Promise((resolve, reject) => {
    const child = execFile(
      binary,
      args,
      { env, timeout, maxBuffer: 64 * 1024 * 1024 },
      (error, stdout, stderr) => {
        if (!error) return resolve(stdout);
        const detail = String(stderr || "").trim() || error.message;
        reject(new Error(`sushiai ${args.slice(0, 2).join(" ")}: ${detail}`));
      },
    );
    child.stdin.on("error", () => {});
    child.stdin.end(input);
  });
}

module.exports = { runSushiai };
