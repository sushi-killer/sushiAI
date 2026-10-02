const path = require("node:path");
const fs = require("node:fs/promises");
const { HERDR_CONTRACT } = require("./herdr-contract.cjs");
const { request, HerdrError, errorDetails } = require("./herdr.cjs");
const { run, quote } = require("./connections.cjs");

const REMOTE_PATH = 'export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; ';

/** What this Herdr build is missing from what sushiAI calls. A different
 * version or a newer protocol is not on the list: Herdr adds to its API, so a
 * build is used as long as everything the contract names is still there. */
function schemaIssues(schema) {
  const issues = [];
  if (!(schema.protocol >= HERDR_CONTRACT.minProtocol))
    issues.push(`CLI protocol ${schema.protocol} is older than supported.`);
  if (schema.schema_version !== HERDR_CONTRACT.schemaVersion)
    issues.push(`CLI schema ${schema.schema_version} is not supported.`);
  const methods = new Set(
    (schema.schemas?.request?.oneOf || []).map(
      (entry) => entry.properties?.method?.const,
    ),
  );
  for (const method of HERDR_CONTRACT.requiredMethods)
    if (!methods.has(method)) issues.push(`CLI is missing ${method}.`);
  for (const name of HERDR_CONTRACT.launchEnvMethods)
    if (!schema.schemas?.request?.$defs?.[name]?.properties?.env)
      issues.push(`CLI is missing ${name}.env.`);
  const events = new Set(
    (schema.schemas?.request?.$defs?.Subscription?.oneOf || []).map(
      (entry) => entry.properties?.type?.const,
    ),
  );
  for (const type of HERDR_CONTRACT.eventTypes)
    if (!events.has(type)) issues.push(`CLI is missing event ${type}.`);
  return issues;
}

const MANAGED_REMOTE = ".local/share/sushiai/herdr";

/** Every other Herdr CLI that might speak the running daemon's protocol: the
 * owner's own one on PATH, then the releases sushiAI installed before. */
async function otherClis({ endpoint, connections, binary }) {
  const ssh = endpoint.startsWith("ssh:");
  let managed = [];
  try {
    managed = ssh
      ? (
          await connections.exec(
            endpoint,
            `for f in "$HOME/${MANAGED_REMOTE}"/*/herdr; do [ -x "$f" ] && printf '%s\\n' "$f"; done; true`,
          )
        )
          .split("\n")
          .map((line) => line.trim())
          .filter(Boolean)
      : connections.herdrInstallDirectory
        ? (
            await fs.readdir(connections.herdrInstallDirectory, {
              withFileTypes: true,
            })
          )
            .filter((entry) => entry.isDirectory())
            .map((entry) =>
              path.join(connections.herdrInstallDirectory, entry.name, "herdr"),
            )
        : [];
  } catch {
    /* No earlier releases. */
  }
  const own = (file) =>
    path.basename(path.dirname(file)) === HERDR_CONTRACT.version;
  // Over SSH, no binary means "the host's own `command -v herdr`".
  return [ssh ? undefined : binary, ...managed.filter((file) => !own(file))];
}

// ponytail: no cache, a mismatched SSH host re-probes candidates on each
// attach; cache the working CLI per endpoint+daemon generation if it shows.
async function checkHerdrCompatibility(options) {
  const status = await checkOnce(options);
  // sushiAI's own pinned CLI cannot attach to a daemon of another protocol
  // (the owner's Herdr is older or newer, or still runs a release sushiAI
  // installed before); a CLI that speaks the daemon's protocol then may.
  // A check of one named CLI (`preferManaged: false`) stays about that CLI.
  if (
    status.compatible ||
    !status.daemon.compatible ||
    options.preferManaged === false
  )
    return status;
  const tried = new Set([status.cli.binary]);
  for (const binary of await otherClis(options)) {
    if (binary !== undefined && (!binary || tried.has(binary))) continue;
    tried.add(binary);
    const other = await checkOnce({ ...options, binary, preferManaged: false });
    if (other.compatible) return other;
  }
  return status;
}

async function checkOnce({
  endpoint,
  connections,
  binary,
  runCli = run,
  rpc = request,
  preferManaged = true,
}) {
  const status = {
    endpoint,
    compatible: false,
    expected: {
      version: HERDR_CONTRACT.version,
      protocol: HERDR_CONTRACT.protocol,
    },
    daemon: { available: false, compatible: false },
    cli: { available: false, compatible: false, stream: false },
    issues: [],
  };
  const daemon = (async () => {
    try {
      const socket = await connections.socket(endpoint);
      const pong = await rpc(socket, "ping");
      status.daemon = {
        available: true,
        compatible: false,
        socket,
        version: pong.version,
        protocol: pong.protocol,
        capabilities: pong.capabilities,
      };
      status.daemon.compatible = pong.protocol >= HERDR_CONTRACT.minProtocol;
      if (!status.daemon.compatible)
        status.issues.push(
          `Daemon protocol ${pong.protocol} is older than supported.`,
        );
    } catch (error) {
      status.daemon.error = errorDetails(error);
      status.issues.push(`Daemon check failed: ${error.message}`);
    }
  })();
  const cli = (async () => {
    try {
      let invoke;
      if (endpoint.startsWith("ssh:")) {
        const managed = `${MANAGED_REMOTE}/${HERDR_CONTRACT.version}/herdr`;
        if (preferManaged || !binary)
          binary = (
            await connections.exec(
              endpoint,
              REMOTE_PATH +
                (preferManaged
                  ? `if [ -x "$HOME/${managed}" ]; then printf '%s\\n' "$HOME/${managed}"; else command -v herdr; fi`
                  : "command -v herdr"),
            )
          ).trim();
        if (!path.posix.isAbsolute(binary) || /[\r\n\0]/.test(binary))
          throw new Error("Herdr CLI is not installed on the SSH host.");
        invoke = (args) =>
          connections.exec(
            endpoint,
            REMOTE_PATH + [binary, ...args].map(quote).join(" "),
          );
      } else {
        if (preferManaged && connections.herdrInstallDirectory) {
          const managed = path.join(
            connections.herdrInstallDirectory,
            HERDR_CONTRACT.version,
            "herdr",
          );
          try {
            await fs.access(managed, fs.constants.X_OK);
            binary = managed;
          } catch (error) {
            if (error.code !== "ENOENT") throw error;
          }
        }
        if (!binary)
          throw new Error("Install Herdr to attach a terminal stream.");
        invoke = (args) => runCli(binary, args);
      }
      const versionText = (await invoke(["--version"])).trim();
      const version = /^herdr\s+(\S+)/.exec(versionText)?.[1];
      if (!version) throw new Error("Invalid Herdr CLI version response.");
      status.cli = {
        available: true,
        compatible: false,
        binary,
        version,
        stream: false,
      };
      const [schemaText, streamHelp] = await Promise.all([
        invoke(["api", "schema", "--json"]),
        invoke([...HERDR_CONTRACT.streamCommand, "--help"]),
      ]);
      const schema = JSON.parse(schemaText);
      const issues = schemaIssues(schema);
      status.cli.protocol = schema.protocol;
      status.cli.stream =
        /Usage:\s+herdr terminal session control\b/.test(streamHelp) &&
        /--cols\b/.test(streamHelp) &&
        /--rows\b/.test(streamHelp);
      if (!status.cli.stream)
        issues.push("CLI does not support terminal session control.");
      status.cli.compatible = issues.length === 0;
      status.issues.push(...issues);
    } catch (error) {
      status.cli.error = errorDetails(error);
      status.issues.push(`CLI check failed: ${error.message}`);
    }
  })();
  await Promise.all([daemon, cli]);
  // The terminal CLI attaches to the daemon itself, so the two must speak one
  // protocol, whatever either number is.
  if (
    status.daemon.compatible &&
    status.cli.compatible &&
    status.daemon.protocol !== status.cli.protocol
  ) {
    status.cli.compatible = false;
    status.issues.push(
      `Terminal CLI ${status.cli.version} (protocol ${status.cli.protocol}) does not match the daemon ${status.daemon.version} (protocol ${status.daemon.protocol}).`,
    );
  }
  status.compatible = status.daemon.compatible && status.cli.compatible;
  return status;
}

async function assertHerdrCompatibility(options) {
  const status = await checkHerdrCompatibility(options);
  if (!status.compatible)
    throw new HerdrError("HERDR_INCOMPATIBLE", status.issues.join(" "), status);
  return status;
}

module.exports = {
  checkHerdrCompatibility,
  assertHerdrCompatibility,
};
