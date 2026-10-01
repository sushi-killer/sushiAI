const os = require("node:os");
const fs = require("node:fs/promises");
const { quote } = require("../connections.cjs");
const { openHerdrStream, detectAgent } = require("../terminal-stream.cjs");
const { terminalEnvironment } = require("../terminal-text.cjs");
const { storeTerminalAttachment } = require("../terminal-attachments.cjs");
const { spawn } = require("node:child_process");
const {
  projectForFolder,
  sessionEnvPrefix,
  sessionAccountId,
  codexAccountFor,
  accountLaunch,
  CODEX_SESSION,
  localCodexAuth,
  modelLaunch,
} = require("../project-session.cjs");

function remoteEnvPayload(env) {
  return Object.entries(env || {})
    .map(([name, value]) => `export ${name}=${quote(value)}`)
    .join("\n");
}

function remoteEnvBootstrap(
  cwd,
  command,
  envPath,
  tokenPath = null,
  settings = "",
  launchShell = "",
) {
  const envSetup = envPath
    ? `trap 'rm -f ${quote(envPath)}${tokenPath ? ` ${quote(tokenPath)}` : ""}' EXIT HUP INT TERM; . ${quote(envPath)}; rm -f ${quote(envPath)};`
    : "";
  const tokenSetup = tokenPath
    ? `exec 3<${quote(tokenPath)}; rm -f ${quote(tokenPath)}; export CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3;`
    : "";
  const shell = `${envSetup} ${tokenSetup} cd ${quote(cwd)} && exec ${launchShell ? `sh -c ${quote(launchShell)}` : command ? quote(command) + settings : '"${SHELL:-/bin/sh}" -l'}`;
  return {
    shell,
  };
}

function uploadRemoteFile(binary, args, payload) {
  return new Promise((resolve, reject) => {
    const proc = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
    let output = "";
    let error = "";
    proc.stdout.setEncoding("utf8");
    proc.stderr.setEncoding("utf8");
    proc.stdout.on("data", (chunk) => (output += chunk));
    proc.stderr.on("data", (chunk) => (error += chunk));
    proc.on("error", reject);
    proc.on("close", (code) => {
      if (code !== 0) return reject(new Error(error || `ssh exited ${code}`));
      resolve(output.trim());
    });
    proc.stdin.end(payload);
  });
}

function remoteFileCommand() {
  return 'umask 077; f=$(mktemp); cat > "$f"; chmod 600 "$f"; printf \'%s\' "$f"';
}

/** Deletes uploaded secret files from a host, best effort: a session that
 * never started must not leave them behind. */
async function removeRemoteFiles(binary, args, paths) {
  const files = paths.filter(Boolean);
  if (!files.length) return;
  const quoted = files.map(
    (path) => `'${String(path).replaceAll("'", "'\\''")}'`,
  );
  await uploadRemoteFile(
    binary,
    [...args, `rm -f ${quoted.join(" ")}`],
    "",
  ).catch(() => {});
}

function claudeFdLaunch(binary, tokenPath) {
  return {
    binary: "/bin/sh",
    args: [
      "-c",
      'exec 3<"$1"; rm -f "$1"; export CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3; shift; exec "$@"',
      "sushiai",
      tokenPath,
      binary,
    ],
  };
}

function registerTerminalIpc({
  handle,
  send,
  app,
  pty,
  getConnections,
  executable,
  directory,
  id,
  terminals,
  terminalPending,
  stageModelSettings,
  stageClaudeAccount,
  resolveClaudeAccount,
  resolveCodexAccount,
  resolveModel,
  codexAuth = localCodexAuth,
  projects,
  sshBinary = "/usr/bin/ssh",
}) {
  // A Herdr pane starts a shell the app does not launch: the values it needs
  // are handed over as a one-shot file whose sourcing is typed into the pane.
  handle(
    "project-session-env",
    async ({
      endpoint,
      cwd,
      claudeAccountId,
      codexAccountId,
      agent,
      modelProfileId,
    }) => {
      if (claudeAccountId) id(claudeAccountId);
      if (codexAccountId) id(codexAccountId);
      if (modelProfileId) id(modelProfileId);
      const connections = getConnections();
      const sshFor = (host) => {
        const remote = connections.get(host);
        return {
          remote,
          args: [...connections.args(remote), "-T", remote.host],
        };
      };
      return sessionEnvPrefix(
        {
          projects,
          connections,
          upload: (host, payload) =>
            uploadRemoteFile(
              sshBinary,
              [...sshFor(host).args, remoteFileCommand()],
              payload,
            ),
          remove: (host, file) =>
            removeRemoteFiles(sshBinary, sshFor(host).args, [file]).catch(
              () => {},
            ),
          resolveAccount: resolveClaudeAccount,
          resolveCodexAccount,
          resolveModel,
          codexAuth,
        },
        {
          endpoint,
          cwd,
          claudeAccountId,
          codexAccountId,
          agent,
          modelProfileId,
        },
      );
    },
  );
  handle("model-launch", async (modelProfileId) => {
    id(modelProfileId);
    return modelLaunch((await resolveModel(modelProfileId)).settings);
  });

  const detectionTimer = setInterval(() => {
    for (const [panelId, entry] of terminals) {
      if (entry.exited || entry.source !== "pty") continue;
      let foreground = "";
      try {
        foreground = entry.remote
          ? entry.command || ""
          : entry.proc.process || "";
      } catch {}
      const agent = detectAgent(foreground, entry.title);
      if (agent !== entry.lastAgent) {
        entry.lastAgent = agent;
        send("terminal-data", { panelId, data: "", agent });
      }
    }
  }, 1200);
  detectionTimer.unref();

  handle(
    "terminal-open",
    async ({
      panelId,
      cwd,
      command,
      cols = 80,
      rows = 24,
      endpoint,
      herdrId,
      modelProfileId,
      claudeAccountId,
      codexAccountId,
      projectId,
    }) => {
      id(panelId);
      if (codexAccountId) id(codexAccountId);
      if (terminals.has(panelId))
        return {
          history: terminals.get(panelId).history,
          exited: terminals.get(panelId).exited,
        };
      const connections = getConnections();
      let remoteEnvBootstrapData;
      if (herdrId) {
        id(herdrId);
        if (terminalPending.has(panelId)) {
          const pending = terminalPending.get(panelId);
          const entry = await pending.promise;
          return { history: entry.history, exited: entry.exited };
        }
        const pending = { cancelled: false, endpoint };
        terminalPending.set(panelId, pending);
        pending.promise = openHerdrStream({
          endpoint,
          panelId,
          target: herdrId,
          cols,
          rows,
          connections,
          binary: executable("herdr"),
          send: (channel, event) => {
            if (!pending.cancelled) send(channel, event);
          },
        });
        try {
          const entry = await pending.promise;
          if (pending.cancelled) {
            entry.proc.kill();
            return { history: "", exited: true };
          }
          terminals.set(panelId, entry);
          entry.opening = pending;
          entry.endpoint = endpoint;
          return { history: "" };
        } finally {
          if (terminalPending.get(panelId) === pending)
            terminalPending.delete(panelId);
        }
      }
      const remote = endpoint?.startsWith("ssh:")
        ? connections.get(endpoint)
        : null;
      if (!remote) directory(cwd);
      else if (typeof cwd !== "string" || !cwd.startsWith("/"))
        throw new Error("Choose an absolute remote project folder.");
      if (
        command &&
        !["claude", "codex", "gemini", "cursor-agent"].includes(command)
      )
        throw new Error("Unsupported agent");
      let remoteCleanup = async () => {};
      let binary = command
        ? executable(command)
        : process.env.SHELL || "/bin/zsh";
      let args = command ? [] : ["-l"];
      if (remote) {
        binary = sshBinary;
        // The renderer may not know the project: the folder says which it is.
        const ownProject = projects
          ? projectId
            ? await projects.get(projectId)
            : await projectForFolder({ projects, connections }, endpoint, cwd)
          : null;
        const sendToHost = ownProject
          ? await projects.sendsValues(ownProject.id, endpoint)
          : false;
        const projectEnv = sendToHost
          ? await projects.environmentFor(ownProject.id, "agent", endpoint)
          : {};
        // The account picked for the session goes with it whether or not the
        // folder has a project; the project's own only where its values go.
        let subscriptionToken = null;
        let accountSettings = "";
        const withheld = Boolean(ownProject) && !sendToHost;
        const accountId =
          command === "claude" && !modelProfileId
            ? sessionAccountId(ownProject, sendToHost, claudeAccountId)
            : undefined;
        // Codex as the picked or the project's account, else on the local
        // login on a host with none, for this session only (CODEX_SESSION).
        let launchShell = "";
        if (command === "codex" && !withheld) {
          const account = await codexAccountFor(
            resolveCodexAccount,
            sessionAccountId(
              ownProject,
              sendToHost,
              codexAccountId,
              "codexAccount",
            ),
            codexAccountId,
            endpoint,
          );
          const auth = account?.auth || (await codexAuth());
          if (auth) {
            projectEnv.SUSHIAI_CODEX_AUTH = auth;
            if (account) projectEnv.SUSHIAI_CODEX_FORCE = "1";
            if (account?.ret) projectEnv.SUSHIAI_CODEX_RETURN = account.ret;
            launchShell = CODEX_SESSION;
          }
        }
        if (accountId && resolveClaudeAccount) {
          // The project's own account without a value yet runs on the host's
          // login; one picked for the session has to work.
          const account = await resolveClaudeAccount(accountId).catch(
            (error) => {
              if (claudeAccountId) throw error;
              return null;
            },
          );
          if (account?.kind === "subscription")
            subscriptionToken = account.value;
          else {
            const launch = accountLaunch(account);
            Object.assign(projectEnv, launch.vars);
            accountSettings = launch.settings;
          }
        }
        const sshArgs = [...connections.args(remote), "-T", remote.host];
        const envPayload = remoteEnvPayload(projectEnv);
        const envPath = envPayload
          ? await uploadRemoteFile(
              binary,
              [...sshArgs, remoteFileCommand()],
              envPayload,
            )
          : null;
        let tokenPath = null;
        try {
          tokenPath = subscriptionToken
            ? await uploadRemoteFile(
                binary,
                [...sshArgs, remoteFileCommand()],
                subscriptionToken,
              )
            : null;
        } catch (error) {
          await removeRemoteFiles(binary, sshArgs, [envPath]);
          throw error;
        }
        remoteCleanup = () =>
          removeRemoteFiles(binary, sshArgs, [envPath, tokenPath]);
        remoteEnvBootstrapData = remoteEnvBootstrap(
          cwd,
          command,
          envPath,
          tokenPath,
          accountSettings,
          launchShell,
        );
        args = [
          ...connections.args(remote),
          "-tt",
          remote.host,
          `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; ${remoteEnvBootstrapData.shell}`,
        ];
      }
      if (!binary)
        throw new Error(
          `${command} is not installed. Install it and sign in from a terminal first.`,
        );
      let modelSettingsPath;
      let accountSettingsPath;
      let accountKeyPath;
      let accountTokenPath;
      const project =
        !remote && projects ? await projects.resolveDirectory(cwd) : null;
      const codexAccount =
        command === "codex" && !remote
          ? await codexAccountFor(
              resolveCodexAccount,
              sessionAccountId(
                projectId && projects ? await projects.get(projectId) : project,
                true,
                codexAccountId,
                "codexAccount",
              ),
              codexAccountId,
            )
          : null;
      if (claudeAccountId && command === "claude" && !remote) {
        const staged = await stageClaudeAccount(claudeAccountId);
        accountSettingsPath = staged.settingsPath;
        accountKeyPath = staged.keyPath;
        accountTokenPath = staged.tokenPath;
        if (staged.kind === "subscription") {
          const launch = claudeFdLaunch(binary, accountTokenPath);
          binary = launch.binary;
          args = launch.args;
        } else {
          args = [...args, "--settings", accountSettingsPath];
        }
      }
      if (modelProfileId && command === "claude" && !remote) {
        modelSettingsPath = await stageModelSettings(modelProfileId);
        args = [...args, "--settings", modelSettingsPath];
      }
      let proc;
      try {
        proc = pty.spawn(binary, args, {
          name: "xterm-256color",
          cols: Math.max(10, Math.min(500, cols)),
          rows: Math.max(3, Math.min(300, rows)),
          cwd: remote ? os.homedir() : cwd,
          env: {
            ...terminalEnvironment(),
            // A remote session's project values travel over ssh, never in
            // the local pty's environment (ssh could forward them).
            ...(!remote && projects && (projectId || project?.id)
              ? await projects.environmentFor(projectId || project.id, "agent")
              : {}),
            ...(codexAccount ? { CODEX_HOME: codexAccount.home } : {}),
          },
        });
      } catch (error) {
        await remoteCleanup();
        throw error;
      }
      const entry = {
        proc,
        history: "",
        exited: false,
        source: "pty",
        lastAgent: undefined,
        title: "",
        remote: !!remote,
        command,
        endpoint,
        modelSettingsPath,
        accountSettingsPath,
        accountKeyPath,
        accountTokenPath,
      };
      terminals.set(panelId, entry);
      proc.onData((data) => {
        const titles = [
          ...data.matchAll(/\x1b\](?:0|2);([^\x07\x1b]*)(?:\x07|\x1b\\)/g),
        ];
        if (titles.length) entry.title = titles.at(-1)[1];
        entry.history = (entry.history + data).slice(-1000000);
        send("terminal-data", { panelId, data });
      });
      proc.onExit(({ exitCode }) => {
        entry.exited = true;
        // ssh can end before the remote shell ever reached its own cleanup.
        void remoteCleanup();
        if (
          entry.modelSettingsPath ||
          entry.accountSettingsPath ||
          entry.accountKeyPath ||
          entry.accountTokenPath
        )
          for (const file of [
            entry.modelSettingsPath,
            entry.modelSettingsPath?.replace(/\.json$/, ".key"),
            entry.accountSettingsPath,
            entry.accountKeyPath,
            entry.accountTokenPath,
          ].filter(Boolean))
            fs.unlink(file).catch(() => {});
        send("terminal-data", {
          panelId,
          exitCode,
          data: `\r\n\x1b[90mProcess exited (${exitCode}). Close this panel to start a new session.\x1b[0m\r\n`,
        });
      });
      return { history: "" };
    },
  );
  handle("terminal-scroll", (panelId, direction, lines, position) => {
    if (
      position &&
      (!Number.isInteger(position.column) ||
        !Number.isInteger(position.row) ||
        position.column < 0 ||
        position.column >= 500 ||
        position.row < 0 ||
        position.row >= 300)
    )
      throw new Error("Invalid terminal mouse position");
    if (
      ["up", "down"].includes(direction) &&
      Number.isInteger(lines) &&
      lines > 0 &&
      lines < 1000
    )
      return terminals
        .get(id(panelId))
        ?.proc.scroll?.(direction, lines, position);
  });
  handle("terminal-write", (panelId, data) => {
    if (typeof data !== "string" || data.length > 1000000)
      throw new Error("Invalid input");
    terminals.get(id(panelId))?.proc.write(data);
  });
  handle("terminal-attach", async ({ panelId, name, data }) =>
    storeTerminalAttachment({
      terminal: terminals.get(id(panelId)),
      name,
      data,
      dataDir: app.getPath("userData"),
      connections: getConnections(),
    }),
  );
  handle("terminal-resize", (panelId, cols, rows) => {
    if (
      !Number.isInteger(cols) ||
      !Number.isInteger(rows) ||
      cols < 10 ||
      rows < 3 ||
      cols > 500 ||
      rows > 300
    )
      return;
    const terminal = terminals.get(id(panelId));
    if (terminal && !terminal.exited) terminal.proc.resize(cols, rows);
  });
  handle("terminal-close", (panelId) => {
    const pending = terminalPending.get(id(panelId));
    if (pending) {
      pending.cancelled = true;
      terminalPending.delete(panelId);
    }
    const terminal = terminals.get(id(panelId));
    if (terminal?.opening) terminal.opening.cancelled = true;
    if (terminal && !terminal.exited) terminal.proc.kill();
    terminals.delete(panelId);
  });

  return {
    close() {
      clearInterval(detectionTimer);
    },
  };
}

module.exports = {
  registerTerminalIpc,
  claudeFdLaunch,
  remoteEnvBootstrap,
  remoteEnvPayload,
  uploadRemoteFile,
  removeRemoteFiles,
  remoteFileCommand,
};
