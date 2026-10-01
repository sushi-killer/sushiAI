const { createHash, randomUUID } = require("node:crypto");
const { normalizeRemote } = require("./projects.cjs");

const quote = (value) => "'" + String(value).replaceAll("'", "'\\''") + "'";
const invalidText = (value) =>
  [...value].some(
    (character) =>
      character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
  );

function sshRemote(value) {
  if (typeof value !== "string" || !value.trim() || invalidText(value))
    throw new Error("Enter an SSH git URL.");
  const url = value.trim();
  let hostname;
  let port = 22;
  let pathname;
  if (url.startsWith("ssh://")) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      throw new Error("Enter a valid SSH git URL.");
    }
    if (parsed.password || parsed.search || parsed.hash)
      throw new Error(
        "An SSH git URL cannot contain a password, query or fragment.",
      );
    hostname = parsed.hostname.replace(/^\[|\]$/g, "");
    port = parsed.port ? Number(parsed.port) : 22;
    pathname = parsed.pathname.slice(1);
    if (parsed.username && !/^[A-Za-z0-9_.-]+$/.test(parsed.username))
      throw new Error("Enter a valid SSH git username.");
  } else {
    if (url.includes("://")) throw new Error("Enter an SSH git URL.");
    const match = url.match(
      /^(?:([A-Za-z0-9_.-]+)@)?([A-Za-z0-9][A-Za-z0-9_.-]*):(.+)$/,
    );
    if (!match)
      throw new Error(
        "Enter an SSH git URL, such as git@host:team/repository.git.",
      );
    hostname = match[2];
    pathname = match[3];
  }
  if (
    !hostname ||
    !/^[A-Za-z0-9][A-Za-z0-9_.:-]*$/.test(hostname) ||
    !pathname ||
    !Number.isInteger(port) ||
    port < 1 ||
    port > 65535
  )
    throw new Error("Enter a valid SSH git host, port and repository path.");
  return {
    url,
    hostname,
    port,
    knownHost: port === 22 ? hostname : "[" + hostname + "]:" + port,
  };
}

function sshFallback(value) {
  if (!/^https?:\/\//i.test(value || "")) return undefined;
  try {
    const url = new URL(value);
    if (!url.hostname || url.search || url.hash || !url.pathname.slice(1))
      return undefined;
    const ssh = "git@" + url.hostname + ":" + url.pathname.slice(1);
    sshRemote(ssh);
    return normalizeRemote(ssh) === normalizeRemote(value) ? ssh : undefined;
  } catch {
    return undefined;
  }
}

function prepareGitUrl(project, override) {
  if (override === undefined || override === "") return project.git.url;
  const remote = sshRemote(override);
  if (normalizeRemote(remote.url) !== normalizeRemote(project.git.url))
    throw new Error(
      "The SSH URL must name the same repository as this project.",
    );
  return remote.url;
}

function gitFailure(text, url, timedOut = false) {
  const transport = /^https?:\/\//i.test(url)
    ? "https"
    : /^(ssh:\/\/|(?:[^@/:]+@)?[^/:]+:)/.test(url)
      ? "ssh"
      : undefined;
  let kind = "other";
  const changed =
    /REMOTE HOST IDENTIFICATION HAS CHANGED|POSSIBLE DNS SPOOFING|Offending .* key|host key .* has changed/i.test(
      text,
    );
  if (timedOut) kind = "network";
  else if (
    changed ||
    /Host key verification failed|No .* host key is known|authenticity of host .* can't be established/i.test(
      text,
    )
  )
    kind = "host-key";
  else if (
    /Remote branch .* not found|couldn't find remote ref|not a valid branch|invalid refspec/i.test(
      text,
    )
  )
    kind = "branch";
  else if (
    /already exists|Permission denied(?! \(publickey)|No space left|Read-only file system|cannot create|could not create|unable to create|not a git checkout|different repository|without an origin/i.test(
      text,
    )
  )
    kind = "path";
  else if (
    /could not read (?:Username|Password)|terminal prompts disabled|Authentication failed|Permission denied \([^)]*publickey|HTTP (?:Basic|Bearer): Access denied|\b(?:401|403)\b|repository .* not found/i.test(
      text,
    )
  )
    kind = "auth";
  else if (
    /Could not resolve|Couldn't resolve|Failed to connect|Connection (?:refused|reset|closed|timed out)|Network is unreachable|unable to access|SSL certificate problem|TLS|Could not read from remote repository/i.test(
      text,
    )
  )
    kind = "network";
  if (!transport) return undefined;
  let safeUrl = url;
  if (transport === "https") {
    try {
      const parsed = new URL(url);
      parsed.username = "";
      parsed.password = "";
      parsed.search = "";
      parsed.hash = "";
      safeUrl = parsed.toString();
    } catch {}
  }
  return {
    kind,
    transport,
    url: safeUrl,
    sshUrl: sshFallback(url),
    ...(changed ? { changed: true } : {}),
  };
}

function gitSshEnv(url) {
  let remote;
  try {
    remote = sshRemote(url);
  } catch {
    return "";
  }
  const command =
    "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=15";
  return [
    "export GIT_SSH_COMMAND=" +
      quote(
        command +
          " -o 'UserKnownHostsFile=~/.ssh/known_hosts ~/.ssh/known_hosts2 ~/.sushiai/git/known_hosts'",
      ),
    'if [ -f "$HOME/.sushiai/git/known_hosts" ] && ssh-keygen -F ' +
      quote(remote.knownHost) +
      ' -f "$HOME/.sushiai/git/known_hosts" >/dev/null 2>&1; then',
    "  GIT_SSH_COMMAND=" +
      quote(
        command +
          " -o UserKnownHostsFile=~/.sushiai/git/known_hosts -o GlobalKnownHostsFile=/dev/null",
      ),
    "fi",
    'if [ -f "$HOME/.sushiai/git/id_ed25519" ]; then',
    '  GIT_SSH_COMMAND="$GIT_SSH_COMMAND"\' -i "$HOME/.sushiai/git/id_ed25519" -o IdentitiesOnly=yes\'',
    "fi",
    "export GIT_SSH_VARIANT=ssh",
    "",
  ].join("\n");
}

const KEY_SCRIPT = [
  "set -e",
  "umask 077",
  'dir="$HOME/.sushiai/git"',
  'for f in "$HOME/.sushiai" "$dir" "$dir/id_ed25519" "$dir/id_ed25519.pub"; do',
  "  if [ -L \"$f\" ]; then echo 'Git key storage cannot be a symbolic link.' >&2; exit 1; fi",
  "done",
  'mkdir -p "$dir"',
  'chmod 700 "$HOME/.sushiai" "$dir"',
  'key="$dir/id_ed25519"',
  "temporary=",
  'trap \'[ -z "$temporary" ] || rm -rf "$temporary"\' EXIT',
  'if [ ! -e "$key" ]; then',
  '  temporary=$(mktemp -d "$dir/key.XXXXXX")',
  "  ssh-keygen -q -t ed25519 -N '' -C sushiai-git -f \"$temporary/id_ed25519\"",
  '  ln "$temporary/id_ed25519" "$key" || [ -f "$key" ]',
  "fi",
  "[ -f \"$key\" ] || { echo 'The Git SSH key is not a regular file.' >&2; exit 1; }",
  'chmod 600 "$key"',
  "public_key=$(ssh-keygen -y -P '' -f \"$key\")",
  '[ -n "$temporary" ] || temporary=$(mktemp -d "$dir/key.XXXXXX")',
  'printf \'%s\\n\' "$public_key" > "$temporary/id_ed25519.pub"',
  'ssh-keygen -lf "$temporary/id_ed25519.pub" -E sha256',
  'mv "$temporary/id_ed25519.pub" "$key.pub"',
  'chmod 600 "$key.pub"',
  "printf 'SUSHIAI_PUBLIC_KEY=%s\\n' \"$public_key\"",
].join("\n");

function knownKeysScript(remote, own = false) {
  const files = own
    ? '"$HOME/.sushiai/git/known_hosts"'
    : '"$HOME/.ssh/known_hosts" "$HOME/.ssh/known_hosts2" /etc/ssh/ssh_known_hosts /etc/ssh/ssh_known_hosts2';
  return [
    "for f in " + files + "; do",
    '  [ ! -f "$f" ] || ssh-keygen -F ' +
      quote(remote.knownHost) +
      ' -f "$f" || true',
    "done",
  ].join("\n");
}

function keysFrom(output, remote, scanned = false) {
  return String(output)
    .split("\n")
    .filter((line) => line && !line.startsWith("#"))
    .map((line) => {
      const fields = line.trim().split(/\s+/);
      if (fields[0]?.startsWith("@")) fields.shift();
      const [host, type, key] = fields;
      if (
        !host ||
        (scanned && host !== remote.knownHost) ||
        !/^(ssh-(?:ed25519|rsa)|ecdsa-sha2-nistp(?:256|384|521))$/.test(
          type || "",
        ) ||
        !/^[A-Za-z0-9+/]+={0,2}$/.test(key || "")
      )
        throw new Error(
          "The Git server returned an unsupported or invalid host key.",
        );
      const fingerprint =
        "SHA256:" +
        createHash("sha256")
          .update(Buffer.from(key, "base64"))
          .digest("base64")
          .replace(/=+$/, "");
      return {
        line: remote.knownHost + " " + type + " " + key,
        key: type + " " + key,
        fingerprint,
      };
    });
}

function changedKeys(known, scanned) {
  return known.some(
    (key) => !scanned.some((candidate) => candidate.key === key.key),
  );
}

function createGitSsh({ connections, projects }) {
  const scans = new Map();
  const queues = new Map();
  const queue = (host, operation) => {
    const run = (queues.get(host) || Promise.resolve()).then(operation);
    queues.set(
      host,
      run.catch(() => {}),
    );
    return run;
  };
  const projectOnHost = async (id, host, url) => {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    const project = await projects.get(id);
    if (!project?.git?.url) throw new Error("This project has no git remote.");
    if (url !== undefined) prepareGitUrl(project, url);
    return project;
  };
  return {
    key: (id, host) =>
      queue(host, async () => {
        await projectOnHost(id, host);
        const output = await connections().exec(host, KEY_SCRIPT, {
          timeout: 90000,
        });
        const publicKey = output.match(/^SUSHIAI_PUBLIC_KEY=(.+)$/m)?.[1];
        const fingerprint = output.match(
          /(SHA256:[A-Za-z0-9+/]+)(?=\s|$)/,
        )?.[1];
        if (!publicKey || !fingerprint)
          throw new Error(
            "The host did not return a Git public key and fingerprint.",
          );
        return { publicKey, fingerprint };
      }),
    scan: (id, host, url) =>
      queue(host, async () => {
        const project = await projectOnHost(id, host, url);
        const remote = sshRemote(url);
        const output = await connections().exec(
          host,
          [
            knownKeysScript(remote),
            "echo SUSHIAI_OWN_KEYS",
            knownKeysScript(remote, true),
            "echo SUSHIAI_SCAN",
            "ssh-keyscan -T 10 -p " +
              remote.port +
              " " +
              quote(remote.hostname),
          ].join("\n"),
          { timeout: 30000 },
        );
        const split = output.indexOf("SUSHIAI_SCAN\n");
        const ownSplit = output.indexOf("SUSHIAI_OWN_KEYS\n");
        if (split < 0 || ownSplit < 0)
          throw new Error("The host did not return a Git server scan.");
        const scanned = keysFrom(
          output.slice(split + "SUSHIAI_SCAN\n".length),
          remote,
          true,
        );
        if (!scanned.length)
          throw new Error(
            "No SSH host keys were returned. Check the Git server hostname, port and network access.",
          );
        const known = keysFrom(output.slice(0, ownSplit), remote);
        const own = keysFrom(
          output.slice(ownSplit + "SUSHIAI_OWN_KEYS\n".length, split),
          remote,
        );
        const scanId = randomUUID();
        for (const [key, value] of scans)
          if (Date.now() - value.at > 10 * 60 * 1000) scans.delete(key);
        if (scans.size >= 256) scans.delete(scans.keys().next().value);
        const changed =
          changedKeys(own, scanned) || changedKeys(known, scanned);
        scans.set(scanId, {
          id,
          host,
          remote,
          scanned,
          own,
          known,
          changed,
          projectRemote: normalizeRemote(project.git.url),
          at: Date.now(),
        });
        return {
          scanId,
          host: remote.knownHost,
          fingerprints: [...new Set(scanned.map((key) => key.fingerprint))],
          ...(changed ? { changed: true } : {}),
        };
      }),
    trust: (id, host, scanId) =>
      queue(host, async () => {
        const project = await projectOnHost(id, host);
        const scan = scans.get(scanId);
        if (
          !scan ||
          scan.id !== id ||
          scan.host !== host ||
          scan.projectRemote !== normalizeRemote(project.git.url) ||
          Date.now() - scan.at > 10 * 60 * 1000
        )
          throw new Error(
            "This Git server scan expired or belongs to another project or host. Scan again.",
          );
        const known = keysFrom(
          await connections().exec(host, knownKeysScript(scan.remote), {
            timeout: 30000,
          }),
          scan.remote,
        );
        if (
          known
            .map((key) => key.key)
            .sort()
            .join("\n") !==
          scan.known
            .map((key) => key.key)
            .sort()
            .join("\n")
        )
          throw new Error(
            "Your SSH trust files changed after this preview. Scan again before trusting it.",
          );
        const own = keysFrom(
          await connections().exec(host, knownKeysScript(scan.remote, true), {
            timeout: 30000,
          }),
          scan.remote,
        );
        if (
          own
            .map((key) => key.key)
            .sort()
            .join("\n") !==
          scan.own
            .map((key) => key.key)
            .sort()
            .join("\n")
        )
          throw new Error(
            "Git server trust changed after this preview. Scan again before trusting it.",
          );
        await connections().exec(
          host,
          [
            "set -e",
            "umask 077",
            'dir="$HOME/.sushiai/git"',
            'for f in "$HOME/.sushiai" "$dir" "$dir/known_hosts"; do',
            "  if [ -L \"$f\" ]; then echo 'Git host key storage cannot be a symbolic link.' >&2; exit 1; fi",
            "done",
            'mkdir -p "$dir"',
            'chmod 700 "$HOME/.sushiai" "$dir"',
            'temporary=$(mktemp "$dir/known_hosts.XXXXXX")',
            'trap \'rm -f "$temporary" "$temporary.old"\' EXIT',
            '[ ! -f "$dir/known_hosts" ] || cat "$dir/known_hosts" > "$temporary"',
            "ssh-keygen -R " +
              quote(scan.remote.knownHost) +
              ' -f "$temporary" >/dev/null',
            'cat >> "$temporary"',
            'chmod 600 "$temporary"',
            'mv "$temporary" "$dir/known_hosts"',
          ].join("\n"),
          {
            input: scan.scanned.map((key) => key.line).join("\n") + "\n",
            timeout: 30000,
          },
        );
        scans.delete(scanId);
      }),
  };
}

module.exports = {
  sshRemote,
  sshFallback,
  prepareGitUrl,
  gitFailure,
  gitSshEnv,
  createGitSsh,
};
