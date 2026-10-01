const path = require("node:path");
const fs = require("node:fs/promises");
const { HERDR_CONTRACT } = require("./herdr-contract.cjs");
const { request, HerdrError, errorDetails } = require("./herdr.cjs");
const { run, quote } = require("./connections.cjs");

const REMOTE_PATH = 'export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; ';

function schemaIssues(schema) {
  const issues = [];
  if (schema.protocol !== HERDR_CONTRACT.protocol)
    issues.push(`CLI protocol ${schema.protocol} is not supported.`);
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

async function checkHerdrCompatibility({
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
      if (pong.version !== HERDR_CONTRACT.version)
        status.issues.push(
          `Daemon version ${pong.version} is not the verified ${HERDR_CONTRACT.version}.`,
        );
      if (pong.protocol !== HERDR_CONTRACT.protocol)
        status.issues.push(
          `Daemon protocol ${pong.protocol} is not supported.`,
        );
      status.daemon.compatible =
        pong.version === HERDR_CONTRACT.version &&
        pong.protocol === HERDR_CONTRACT.protocol;
    } catch (error) {
      status.daemon.error = errorDetails(error);
      status.issues.push(`Daemon check failed: ${error.message}`);
    }
  })();
  const cli = (async () => {
    try {
      let invoke;
      if (endpoint.startsWith("ssh:")) {
        const managed = `.local/share/sushiai/herdr/${HERDR_CONTRACT.version}/herdr`;
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
      if (version !== HERDR_CONTRACT.version)
        issues.push(
          `CLI version ${version} is not the verified ${HERDR_CONTRACT.version}.`,
        );
      status.cli.compatible = issues.length === 0;
      status.issues.push(...issues);
    } catch (error) {
      status.cli.error = errorDetails(error);
      status.issues.push(`CLI check failed: ${error.message}`);
    }
  })();
  await Promise.all([daemon, cli]);
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
