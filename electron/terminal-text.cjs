const { StringDecoder } = require("node:string_decoder");

// GUI launches need not inherit a UTF-8 locale from a parent shell.
function terminalEnvironment(env = process.env) {
  const result = { ...env, TERM: "xterm-256color", COLORTERM: "truecolor" };
  const utf8 = (value) => /utf-?8/i.test(value || "");
  if (!utf8(result.LANG)) result.LANG = "en_US.UTF-8";
  if (result.LC_ALL && !utf8(result.LC_ALL)) result.LC_ALL = result.LANG;
  if (!utf8(result.LC_CTYPE)) result.LC_CTYPE = result.LANG;
  return result;
}

function createFrameDecoder(maxBytes = Infinity) {
  let decoder = new StringDecoder("utf8");
  return (bytes, full = false) => {
    const padding = typeof bytes === "string" ? bytes.indexOf("=") : -1;
    if (
      typeof bytes !== "string" ||
      bytes.length % 4 !== 0 ||
      /[^A-Za-z0-9+/=]/.test(bytes) ||
      (padding >= 0 && !/^={1,2}$/.test(bytes.slice(padding)))
    )
      throw new Error("Invalid terminal frame encoding.");
    const data = Buffer.from(bytes, "base64");
    if (data.length > maxBytes)
      throw new Error("Terminal frame exceeds the supported size.");
    if (data.toString("base64") !== bytes)
      throw new Error("Invalid terminal frame encoding.");
    if (full) decoder = new StringDecoder("utf8");
    return decoder.write(data);
  };
}

function herdrLaunchParams(method, params) {
  if (!["workspace.create", "pane.split", "worktree.create"].includes(method))
    return params;
  // Herdr is a separate, long-lived process. The Electron environment does not
  // reach its shells: explicitly pass a locale on every shell-creation RPC.
  const env = terminalEnvironment(params.env || {});
  env.LC_ALL ||= env.LC_CTYPE;
  return { ...params, env };
}
module.exports = { terminalEnvironment, createFrameDecoder, herdrLaunchParams };
