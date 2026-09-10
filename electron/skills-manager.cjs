const path = require("node:path");
const fs = require("node:fs/promises");
const { randomUUID } = require("node:crypto");
const { spawn } = require("node:child_process");

const ACTIONS = new Set(["enable", "disable", "delete"]);
const PLUGIN_KEY =
  /^[A-Za-z0-9][A-Za-z0-9._:/-]*@[A-Za-z0-9][A-Za-z0-9._:/-]*$/;

function inside(target, parent) {
  const relative = path.relative(parent, target);
  return Boolean(
    relative && !relative.startsWith("..") && !path.isAbsolute(relative),
  );
}

function configString(value) {
  const text = String(value || "").trim();
  if (text.startsWith('"') && text.endsWith('"')) {
    try {
      return JSON.parse(text);
    } catch {
      return text.slice(1, -1);
    }
  }
  if (text.startsWith("'") && text.endsWith("'")) return text.slice(1, -1);
  return text;
}

function tomlString(value) {
  return JSON.stringify(String(value));
}

function resolveConfiguredPath(value, baseDir = process.cwd()) {
  const text = String(value || "");
  const expanded = text.startsWith("~/")
    ? path.join(baseDir, text.slice(2))
    : text;
  return path.resolve(
    path.isAbsolute(expanded) ? expanded : path.join(baseDir, expanded),
  );
}

function splitLines(source) {
  return {
    lines: String(source || "").split(/\r?\n/),
    newline: String(source || "").includes("\r\n") ? "\r\n" : "\n",
  };
}

function skillConfigBlocks(lines) {
  const starts = [];
  for (let index = 0; index < lines.length; index++)
    if (lines[index].trim() === "[[skills.config]]") starts.push(index);
  return starts.map((start, offset) => ({
    start,
    end: starts[offset + 1] ?? lines.length,
  }));
}

function updateCodexSkillConfigText(
  source,
  skillPath,
  enabled,
  baseDir = process.cwd(),
) {
  const { lines, newline } = splitLines(source);
  const wanted = path.resolve(skillPath);
  for (const block of skillConfigBlocks(lines)) {
    let configuredPath;
    let enabledIndex = -1;
    let pathIndex = -1;
    for (let index = block.start + 1; index < block.end; index++) {
      const pathValue = lines[index].match(
        /^\s*path\s*=\s*("(?:\\.|[^"\\])*"|'[^']*')\s*(?:#.*)?$/,
      );
      if (pathValue) {
        configuredPath = configString(pathValue[1]);
        pathIndex = index;
      }
      if (/^\s*enabled\s*=\s*(?:true|false)\s*(?:#.*)?$/.test(lines[index]))
        enabledIndex = index;
    }
    if (!configuredPath) continue;
    const normalized = resolveConfiguredPath(configuredPath, baseDir);
    if (normalized !== wanted) continue;
    if (enabledIndex >= 0) {
      lines[enabledIndex] = lines[enabledIndex].replace(
        /\b(?:true|false)\b/,
        enabled ? "true" : "false",
      );
    } else {
      lines.splice(
        pathIndex >= 0 ? pathIndex + 1 : block.start + 1,
        0,
        `enabled = ${enabled}`,
      );
    }
    return { text: lines.join(newline), changed: true };
  }
  const suffix =
    String(source || "").length && !String(source).endsWith(newline)
      ? newline
      : "";
  const prefix = String(source || "").length ? suffix : "";
  const block = [
    "[[skills.config]]",
    `path = ${tomlString(wanted)}`,
    `enabled = ${enabled}`,
  ].join(newline);
  return { text: `${source || ""}${prefix}${block}${newline}`, changed: true };
}

function removeCodexSkillConfigText(
  source,
  skillPath,
  baseDir = process.cwd(),
) {
  const { lines, newline } = splitLines(source);
  const wanted = path.resolve(skillPath);
  const blocks = skillConfigBlocks(lines);
  for (const block of blocks) {
    let configuredPath;
    for (let index = block.start + 1; index < block.end; index++) {
      const pathValue = lines[index].match(
        /^\s*path\s*=\s*("(?:\\.|[^"\\])*"|'[^']*')\s*(?:#.*)?$/,
      );
      if (pathValue) configuredPath = configString(pathValue[1]);
    }
    if (
      configuredPath &&
      resolveConfiguredPath(configuredPath, baseDir) === wanted
    ) {
      lines.splice(block.start, block.end - block.start);
      return { text: lines.join(newline), changed: true };
    }
  }
  return { text: String(source || ""), changed: false };
}

function updateCodexPluginText(source, pluginKey, enabled) {
  const { lines, newline } = splitLines(source);
  const header = `[plugins."${pluginKey}"]`;
  const start = lines.findIndex((line) => line.trim() === header);
  if (start < 0)
    throw new Error(`Codex plugin config was not found: ${pluginKey}`);
  let end = lines.length;
  for (let index = start + 1; index < lines.length; index++) {
    if (/^\s*\[/.test(lines[index])) {
      end = index;
      break;
    }
  }
  for (let index = start + 1; index < end; index++) {
    if (/^\s*enabled\s*=\s*(?:true|false)\s*(?:#.*)?$/.test(lines[index])) {
      lines[index] = lines[index].replace(
        /\b(?:true|false)\b/,
        enabled ? "true" : "false",
      );
      return { text: lines.join(newline), changed: true };
    }
  }
  lines.splice(start + 1, 0, `enabled = ${enabled}`);
  return { text: lines.join(newline), changed: true };
}

async function readText(filePath, fallback = "") {
  try {
    return await fs.readFile(filePath, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return fallback;
    throw error;
  }
}

async function writeAtomic(filePath, text, mode = 0o600) {
  await fs.mkdir(path.dirname(filePath), { recursive: true, mode: 0o700 });
  let fileMode = mode;
  try {
    fileMode = (await fs.stat(filePath)).mode & 0o777;
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const temporary = `${filePath}.sushiai-${process.pid}-${randomUUID()}`;
  try {
    await fs.writeFile(temporary, text, { encoding: "utf8", mode: fileMode });
    await fs.chmod(temporary, fileMode);
    await fs.rename(temporary, filePath);
  } finally {
    await fs.rm(temporary, { force: true }).catch(() => {});
  }
}

async function updateClaudeSkillOverride(home, name, enabled) {
  const settingsPath = path.join(home, ".claude/settings.json");
  const source = await readText(settingsPath, "{}\n");
  let settings;
  try {
    settings = JSON.parse(source);
  } catch {
    throw new Error("Claude settings.json is not valid JSON.");
  }
  if (!settings || typeof settings !== "object" || Array.isArray(settings))
    throw new Error("Claude settings.json must contain an object.");
  const overrides =
    settings.skillOverrides && typeof settings.skillOverrides === "object"
      ? { ...settings.skillOverrides }
      : {};
  const matchingKeys = Object.keys(overrides).filter(
    (key) => key.toLowerCase() === String(name).toLowerCase(),
  );
  if (enabled) {
    for (const key of matchingKeys) delete overrides[key];
  } else {
    const key = matchingKeys[0] || String(name);
    overrides[key] = "off";
  }
  settings.skillOverrides = overrides;
  await writeAtomic(settingsPath, `${JSON.stringify(settings, null, 2)}\n`);
  return { path: settingsPath, changed: true };
}

async function updateCodexSkillConfig(home, skillPath, enabled) {
  const configPath = path.join(home, ".codex/config.toml");
  const source = await readText(configPath, "");
  const result = updateCodexSkillConfigText(source, skillPath, enabled, home);
  await writeAtomic(configPath, result.text);
  return { path: configPath, changed: result.changed };
}

async function removeCodexSkillConfig(home, skillPath) {
  const configPath = path.join(home, ".codex/config.toml");
  const source = await readText(configPath, "");
  const result = removeCodexSkillConfigText(source, skillPath, home);
  if (result.changed) await writeAtomic(configPath, result.text);
  return { path: configPath, changed: result.changed };
}

async function updateCodexPlugin(home, pluginKey, enabled) {
  const configPath = path.join(home, ".codex/config.toml");
  const source = await readText(configPath, "");
  const result = updateCodexPluginText(source, pluginKey, enabled);
  await writeAtomic(configPath, result.text);
  return { path: configPath, changed: result.changed };
}

function runCommand(command, args, home) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: home,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    const collect = (chunk) => {
      if (output.length < 10000) output += chunk.toString("utf8");
    };
    child.stdout.on("data", collect);
    child.stderr.on("data", collect);
    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      reject(new Error(`${path.basename(command)} did not finish in time.`));
    }, 30000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (code === 0) resolve(output.trim());
      else
        reject(
          new Error(
            `${path.basename(command)} failed${output.trim() ? `: ${output.trim().slice(-1200)}` : "."}`,
          ),
        );
    });
  });
}

function itemPath(home, item) {
  if (typeof item?.path !== "string" || !item.path)
    throw new Error("This skill has no local file path.");
  const value = item.path.startsWith("~/")
    ? path.join(home, item.path.slice(2))
    : item.path;
  if (!path.isAbsolute(value)) throw new Error("Skill path must be absolute.");
  return path.resolve(value);
}

function localRootFor(home, item) {
  const relative =
    item.provider === "Claude"
      ? ".claude/skills"
      : item.provider === "Codex"
        ? ".codex/skills"
        : item.provider === "Agent"
          ? ".agents/skills"
          : null;
  return relative ? path.join(home, relative) : null;
}

async function deleteLocalSkill(home, item, shell) {
  const filePath = await validateLocalSkillPath(home, item);
  const root = localRootFor(home, item);
  if (
    !root ||
    !inside(filePath, root) ||
    path.basename(filePath).toLowerCase() !== "skill.md"
  )
    throw new Error("Only skills from the managed local roots can be deleted.");
  const directory = path.dirname(filePath);
  if (path.resolve(directory) === path.resolve(root))
    throw new Error(
      "The root SKILL.md is protected; move it into its own skill folder first.",
    );
  if (directory.split(path.sep).includes(".system"))
    throw new Error(
      "System skills are protected; disable them instead of deleting them.",
    );
  const stat = await fs.lstat(directory).catch(() => null);
  if (!stat?.isDirectory())
    throw new Error("The skill folder no longer exists.");
  if (stat.isSymbolicLink())
    throw new Error(
      "Symlinked skill folders are protected from deletion here.",
    );
  if (!shell?.trashItem) throw new Error("The system trash is unavailable.");
  await shell.trashItem(directory);
  return { path: directory, changed: true };
}

async function validateLocalSkillPath(home, item) {
  const filePath = itemPath(home, item);
  const root = localRootFor(home, item);
  if (
    !root ||
    !inside(filePath, root) ||
    path.basename(filePath).toLowerCase() !== "skill.md"
  )
    throw new Error("Only skills from the managed local roots can be changed.");
  const stat = await fs.stat(filePath).catch(() => null);
  if (!stat?.isFile()) throw new Error("The skill file no longer exists.");
  return filePath;
}

function pluginScope(item) {
  return ["user", "project", "local"].includes(item.pluginScope)
    ? item.pluginScope
    : "user";
}

async function manageSkill({ home, action, item, shell, executable }) {
  const resolvedHome = path.resolve(home);
  if (!ACTIONS.has(action)) throw new Error("Unknown skill action.");
  if (!item || typeof item !== "object") throw new Error("Invalid skill.");
  if (!["Local", "Plugin"].includes(item.source))
    throw new Error("External skills are managed by their own harness.");

  if (item.source === "Plugin") {
    if (typeof item.plugin !== "string" || !PLUGIN_KEY.test(item.plugin))
      throw new Error("This plugin has no safe provider identifier.");
    if (item.provider === "Claude") {
      const command = executable?.("claude");
      if (!command) throw new Error("Claude CLI was not found on this device.");
      if (action === "delete") {
        await runCommand(
          command,
          ["plugin", "uninstall", item.plugin, "--scope", pluginScope(item)],
          resolvedHome,
        );
        return {
          action,
          changed: true,
          message: `Uninstalled ${item.plugin}.`,
        };
      }
      await runCommand(
        command,
        [
          "plugin",
          action === "disable" ? "disable" : "enable",
          item.plugin,
          "--scope",
          pluginScope(item),
        ],
        resolvedHome,
      );
      return {
        action,
        changed: true,
        message: `${action === "disable" ? "Disabled" : "Enabled"} ${item.plugin}.`,
      };
    }
    if (item.provider === "Codex") {
      if (action === "delete") {
        const command = executable?.("codex");
        if (!command)
          throw new Error("Codex CLI was not found on this device.");
        await runCommand(
          command,
          ["plugin", "remove", item.plugin],
          resolvedHome,
        );
        return { action, changed: true, message: `Removed ${item.plugin}.` };
      }
      if (item.disabledBy === "skill-config")
        await updateCodexSkillConfig(
          resolvedHome,
          itemPath(resolvedHome, item),
          action === "enable",
        );
      else
        await updateCodexPlugin(resolvedHome, item.plugin, action === "enable");
      return {
        action,
        changed: true,
        message:
          item.disabledBy === "skill-config"
            ? `${action === "disable" ? "Disabled" : "Enabled"} ${item.name}.`
            : `${action === "disable" ? "Disabled" : "Enabled"} ${item.plugin}.`,
      };
    }
    throw new Error("This plugin provider is read-only in sushiAI.");
  }

  if (action === "delete") {
    const result = await deleteLocalSkill(resolvedHome, item, shell);
    if (item.provider === "Codex" || item.provider === "Agent")
      await removeCodexSkillConfig(resolvedHome, itemPath(resolvedHome, item));
    return {
      action,
      changed: result.changed,
      message: `Moved ${item.name} to the Trash.`,
    };
  }
  const filePath = await validateLocalSkillPath(resolvedHome, item);
  if (item.provider === "Claude")
    await updateClaudeSkillOverride(
      resolvedHome,
      item.name,
      action === "enable",
    );
  else if (item.provider === "Codex" || item.provider === "Agent")
    await updateCodexSkillConfig(resolvedHome, filePath, action === "enable");
  else throw new Error("This skill provider is read-only in sushiAI.");
  return {
    action,
    changed: true,
    message: `${action === "disable" ? "Disabled" : "Enabled"} ${item.name}.`,
  };
}

module.exports = {
  manageSkill,
  removeCodexSkillConfigText,
  updateClaudeSkillOverride,
  updateCodexPluginText,
  updateCodexSkillConfigText,
};
