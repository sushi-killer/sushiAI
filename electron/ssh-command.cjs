"use strict";

// The one command string sent to a host's login shell for a program argv. The
// login shell may be anything (sh, bash, zsh, fish, csh), and they quote
// differently, so the string holds nothing but fixed text and base64: every
// shell reads it the same way. The base64 decodes to POSIX-quoted words that
// /bin/sh evals, so the words that arrive are exactly the argv shown to the
// owner.

const quote = (text) => "'" + String(text).replaceAll("'", "'\\''") + "'";

// A single command-line argument may not exceed 128 KiB on Linux.
const MAX_COMMAND_CHARS = 100000;

function remoteCommand(argv) {
  const b64 = Buffer.from(argv.map(quote).join(" "), "utf8").toString("base64");
  const decode =
    "{ base64 -d 2>/dev/null || base64 -D 2>/dev/null || openssl base64 -d; }";
  const command = `exec /bin/sh -c 'eval "$(printf %s ${b64} | ${decode})"'`;
  if (command.length > MAX_COMMAND_CHARS)
    throw new Error("The command is too long to send to a host.");
  return command;
}

module.exports = { remoteCommand, MAX_COMMAND_CHARS };
