// What counts as a secret, and how a project's local files are read into a
// project. The main process owns this: values read from disk never travel to
// the renderer, so the renderer cannot be the one deciding what is secret.

// Names containing one of these are secret wherever it appears.
const SECRET_SUBSTRINGS =
  /token|secret|passw(or)?d|credential|bearer|private|cert|auth|apikey|signature|session|cookie|webhook/i;
// Short words only count as whole words (SECRET_KEY_BASE, db_key, DSN), so
// that KEYBOARD or COMPASS stay plain.
const SECRET_WORDS = new Set([
  "key",
  "keys",
  "pass",
  "dsn",
  "pwd",
  "jwt",
  "pat",
  "sk",
]);

function words(name) {
  return String(name)
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
}

function isSecretName(name) {
  const text = String(name || "");
  if (SECRET_SUBSTRINGS.test(text)) return true;
  return words(text).some((word) => SECRET_WORDS.has(word));
}

/** A value that carries a credential whatever its name says: a URL with
 * `user:password@`, or a private key block. */
function isSecretValue(value) {
  const text = String(value || "");
  // `scheme://user:password@host`, and `scheme://:password@host` too.
  if (/^[a-z][a-z0-9+.-]*:\/\/[^/\s:@]*:[^/\s@]+@/i.test(text)) return true;
  // Credentials that say what they are by how they begin.
  if (
    /^(sk_live_|sk_test_|sk-|ghp_|gho_|ghu_|ghs_|github_pat_|xox[abpr]-|AKIA[0-9A-Z]{8})/.test(
      text,
    )
  )
    return true;
  if (/hooks\.slack\.com\/services\//i.test(text)) return true;
  return /-----BEGIN [A-Z ]*PRIVATE KEY-----/.test(text);
}

function isSecret(name, value = "") {
  return isSecretName(name) || isSecretValue(value);
}

function closingQuote(value, quote) {
  for (let index = 0; index < value.length; index++) {
    if (quote === '"' && value[index] === "\\") {
      index++;
      continue;
    }
    if (value[index] === quote) return index;
  }
  return -1;
}

function parseEnv(source) {
  const entries = [];
  let index = 0;
  while (index < source.length) {
    while (index < source.length && /\s/.test(source[index])) index++;
    if (index >= source.length) break;
    if (source[index] === "#") {
      while (index < source.length && source[index] !== "\n") index++;
      continue;
    }
    const lineEnd = source.indexOf("\n", index);
    const end = lineEnd < 0 ? source.length : lineEnd;
    let line = source.slice(index, end).replace(/\r$/, "").trim();
    index = lineEnd < 0 ? source.length : lineEnd + 1;
    line = line.replace(/^export\s+/, "");
    const match = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*/.exec(line);
    if (!match) continue;
    const name = match[1];
    let value = line.slice(match[0].length);
    const quote = value[0];
    if (quote === "'" || quote === '"' || quote === "`") {
      value = value.slice(1);
      let closing = closingQuote(value, quote);
      while (closing < 0 && index < source.length) {
        const nextEnd = source.indexOf("\n", index);
        const next =
          nextEnd < 0 ? source.slice(index) : source.slice(index, nextEnd);
        value += `\n${next.replace(/\r$/, "")}`;
        index = nextEnd < 0 ? source.length : nextEnd + 1;
        closing = closingQuote(value, quote);
      }
      if (closing >= 0) value = value.slice(0, closing);
      if (quote === '"')
        value = value.replace(/\\(["\\$`])/g, "$1").replace(/\\n/g, "\n");
    } else {
      value = value.replace(/\s+#.*$/, "").trimEnd();
    }
    entries.push({ name, value });
  }
  return entries;
}

/** Merge `.env.example` < `.env` < `.env.local`; a later file wins per key.
 * A secret's value in the example is a placeholder and is dropped. Each
 * entry says whether it is a secret; the owner can opt a name out later. */
function mergeEnvSources(files) {
  const merged = new Map();
  for (const [source, real] of [
    [files.example, false],
    [files.env, true],
    [files.local, true],
  ]) {
    if (!source) continue;
    for (const entry of parseEnv(source)) {
      const secret = isSecret(entry.name, entry.value);
      merged.set(entry.name, {
        name: entry.name,
        secret,
        value: secret && !real ? "" : entry.value,
      });
    }
  }
  return [...merged.values()];
}

const REFERENCE = /^\$\{[A-Za-z_][A-Za-z0-9_]*\}$/;

function variableName(...parts) {
  const name = parts
    .join("_")
    .replace(/[^A-Za-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .toUpperCase();
  return /^[A-Za-z_]/.test(name) ? name : `_${name}`;
}

/** Replace literal secrets in MCP `env` and `headers` with `${VAR}`
 * references. Returns the cleaned servers plus the secret values to store
 * under those variable names; nothing secret remains in `servers`. `taken`
 * maps existing variable names to their known values so equal values reuse
 * the name and differing ones get a server-specific name. */
function secretizeMcpServers(servers, taken = {}) {
  const secrets = {};
  const claim = (preferred, server, value) => {
    const specific = variableName(server, preferred);
    for (const name of [preferred, specific]) {
      const known = Object.hasOwn(secrets, name) ? secrets[name] : taken[name];
      if (known === undefined || known === value) {
        secrets[name] = value;
        return name;
      }
    }
    let at = 2;
    while (Object.hasOwn(secrets, `${specific}_${at}`)) at++;
    secrets[`${specific}_${at}`] = value;
    return `${specific}_${at}`;
  };
  const clean = (server, map) => {
    if (!map || typeof map !== "object" || Array.isArray(map)) return map;
    const out = {};
    for (const [key, raw] of Object.entries(map)) {
      if (typeof raw !== "string" || !raw || !isSecret(key, raw)) {
        out[key] = raw;
        continue;
      }
      const prefix = /^(bearer|basic)\s+/i.exec(raw)?.[0] || "";
      const value = raw.slice(prefix.length);
      if (!value || REFERENCE.test(value) || value.includes("${")) {
        out[key] = raw;
        continue;
      }
      out[key] = `${prefix}\${${claim(variableName(key), server, value)}}`;
    }
    return out;
  };
  // A command-line argument that is a credential, or the value of a flag
  // that names one (`--api-key=x`, `--token x`).
  const cleanArgs = (server, args) => {
    if (!Array.isArray(args)) return args;
    return args.map((raw, at) => {
      if (typeof raw !== "string" || !raw || raw.includes("${")) return raw;
      const flag = /^(--?[A-Za-z][\w-]*)=([\s\S]*)$/.exec(raw);
      if (flag) {
        const label = flag[1].replace(/^-+/, "");
        return flag[2] && isSecret(label, flag[2])
          ? `${flag[1]}=\${${claim(variableName(label), server, flag[2])}}`
          : raw;
      }
      const before = typeof args[at - 1] === "string" ? args[at - 1] : "";
      const named =
        /^--?[A-Za-z]/.test(before) &&
        !before.includes("=") &&
        !raw.startsWith("-") &&
        isSecretName(before.replace(/^-+/, ""));
      return named || isSecretValue(raw)
        ? `\${${claim(variableName(named ? before.replace(/^-+/, "") : "arg", at), server, raw)}}`
        : raw;
    });
  };
  // A URL's password and its credential-looking query parameters.
  const cleanUrl = (server, raw) => {
    if (typeof raw !== "string" || !raw || raw.includes("${")) return raw;
    let url = raw.replace(
      /^([a-z][a-z0-9+.-]*:\/\/[^/\s:@]*:)([^/\s@]+)(@)/i,
      (_, head, password, tail) =>
        `${head}\${${claim("URL_PASSWORD", server, password)}}${tail}`,
    );
    url = url.replace(/([?&])([^=&#]+)=([^&#]*)/g, (whole, mark, key, value) =>
      value && !value.includes("${") && isSecret(key, value)
        ? `${mark}${key}=\${${claim(variableName(key), server, value)}}`
        : whole,
    );
    return url;
  };
  const result = {};
  for (const [name, server] of Object.entries(servers)) {
    const next = { ...server };
    if ("env" in next) next.env = clean(name, next.env);
    if ("headers" in next) next.headers = clean(name, next.headers);
    if ("args" in next) next.args = cleanArgs(name, next.args);
    if ("url" in next) next.url = cleanUrl(name, next.url);
    result[name] = next;
  }
  return { servers: result, secrets };
}

module.exports = {
  isSecretName,
  isSecretValue,
  isSecret,
  parseEnv,
  mergeEnvSources,
  secretizeMcpServers,
};
