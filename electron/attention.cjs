const fs = require("node:fs/promises");
const path = require("node:path");
const { existsSync } = require("node:fs");
const { Tray, Menu, Notification, nativeImage } = require("electron");

const DEFAULT_PREFERENCES = { runInMenuBar: true, notifications: true };

/** Falls back to a default for anything missing, non-boolean or unknown -
 * a corrupt or hand-edited preferences file never blocks startup. */
function normalizePreferences(value) {
  const result = { ...DEFAULT_PREFERENCES };
  if (value && typeof value === "object" && !Array.isArray(value))
    for (const key of Object.keys(DEFAULT_PREFERENCES))
      if (typeof value[key] === "boolean") result[key] = value[key];
  return result;
}

/** Whether the window's `close` event should hide the window or let it
 * proceed: only intercepted while the app keeps running in the menu bar and
 * this isn't a real quit. */
function closeAction({ quitting, runInMenuBar }) {
  return !quitting && runInMenuBar ? "hide" : "close";
}

/** Which of the three marks the menu bar wears. Something waiting for you
 * outranks something merely running, and a bad count reads as quiet. */
function trayState(waiting, working) {
  const safe = (value) =>
    typeof value === "number" && Number.isFinite(value) && value > 0;
  if (safe(waiting)) return "attention";
  if (safe(working)) return "working";
  return "idle";
}

/** The tray's title badge: empty clears it, otherwise the plain count. */
function trayTitle(count) {
  return Number.isFinite(count) && count > 0 ? String(Math.trunc(count)) : "";
}

function boundedString(value, max) {
  return typeof value === "string" && value.length > 0 && value.length <= max;
}

/** Validates a notice at the IPC trust boundary - the renderer's input is
 * never assumed to match the type it claims. Throws on anything invalid. */
function validateNotice(notice) {
  if (!notice || typeof notice !== "object" || Array.isArray(notice))
    throw new Error("Invalid attention notice.");
  const { workspaceId, panelId, title, body } = notice;
  if (!boundedString(workspaceId, 200))
    throw new Error("Invalid attention workspace ID.");
  if (!boundedString(panelId, 200))
    throw new Error("Invalid attention panel ID.");
  if (!boundedString(title, 120))
    throw new Error("Invalid attention notice title.");
  if (!boundedString(body, 300))
    throw new Error("Invalid attention notice body.");
  return { workspaceId, panelId, title, body };
}

/** Registers the attention/preferences IPC surface and owns the tray, the
 * Dock badge and live notifications. Mirrors the other `register*Ipc`
 * modules: `handle`/`send` are main.cjs's sender-checked wrappers. */
function registerAttentionIpc({
  handle,
  send,
  app,
  getMainWindow,
  userDataDir,
  trayIconPath,
}) {
  const preferencesFile = path.join(userDataDir, "app-preferences.json");
  let preferences = { ...DEFAULT_PREFERENCES };
  let tray = null;
  let badgeCount = 0;
  let workingCount = 0;
  const trayIcons = new Map();
  let quitting = false;
  const notifications = new Set();

  function showWindow() {
    const win = getMainWindow();
    if (!win || win.isDestroyed()) return;
    if (win.isMinimized()) win.restore();
    win.show();
    win.focus();
  }

  /** The state's mark, falling back to the plain one: a build that shipped
   * only the base icon still gets a tray. */
  function trayImage(state) {
    if (!trayIcons.has(state)) {
      const file =
        state === "idle"
          ? trayIconPath
          : trayIconPath.replace(/\.png$/, `-${state}.png`);
      const icon = nativeImage.createFromPath(
        existsSync(file) ? file : trayIconPath,
      );
      icon.setTemplateImage(true);
      trayIcons.set(state, icon);
    }
    return trayIcons.get(state);
  }

  function createTray() {
    if (tray || !existsSync(trayIconPath)) return;
    tray = new Tray(trayImage(trayState(badgeCount, workingCount)));
    tray.setToolTip("sushiAI");
    tray.setTitle(trayTitle(badgeCount));
    tray.setContextMenu(
      Menu.buildFromTemplate([
        { label: "Open", click: () => showWindow() },
        { type: "separator" },
        {
          label: "Quit",
          click: () => {
            quitting = true;
            app.quit();
          },
        },
      ]),
    );
  }

  function destroyTray() {
    tray?.destroy();
    tray = null;
  }

  function syncTray() {
    if (preferences.runInMenuBar) createTray();
    else destroyTray();
  }

  async function save() {
    await fs.mkdir(path.dirname(preferencesFile), { recursive: true });
    await fs.writeFile(`${preferencesFile}.tmp`, JSON.stringify(preferences), {
      mode: 0o600,
    });
    await fs.rename(`${preferencesFile}.tmp`, preferencesFile);
  }

  async function init() {
    try {
      preferences = normalizePreferences(
        JSON.parse(await fs.readFile(preferencesFile, "utf8")),
      );
    } catch {
      preferences = { ...DEFAULT_PREFERENCES };
    }
    syncTray();
    return preferences;
  }

  async function setPreferences(patch) {
    const merged = { ...preferences };
    if (patch && typeof patch === "object" && !Array.isArray(patch))
      for (const key of Object.keys(DEFAULT_PREFERENCES))
        if (Object.hasOwn(patch, key) && typeof patch[key] === "boolean")
          merged[key] = patch[key];
    preferences = merged;
    await save();
    syncTray();
    return preferences;
  }

  async function setBadge(count, working = 0) {
    if (typeof count !== "number" || !Number.isFinite(count) || count < 0)
      throw new Error("Invalid attention badge count.");
    if (typeof working !== "number" || !Number.isFinite(working) || working < 0)
      throw new Error("Invalid attention working count.");
    badgeCount = Math.trunc(count);
    workingCount = Math.trunc(working);
    app.setBadgeCount(badgeCount);
    tray?.setTitle(trayTitle(badgeCount));
    tray?.setImage(trayImage(trayState(badgeCount, workingCount)));
  }

  async function notify(rawNotice) {
    const notice = validateNotice(rawNotice);
    if (!preferences.notifications) return;
    const win = getMainWindow();
    if (win && !win.isDestroyed() && win.isVisible() && win.isFocused()) return;
    const notification = new Notification({
      title: notice.title,
      body: notice.body,
    });
    notifications.add(notification);
    const cleanup = () => notifications.delete(notification);
    notification.on("click", () => {
      cleanup();
      showWindow();
      send("attention-open", {
        workspaceId: notice.workspaceId,
        panelId: notice.panelId,
      });
    });
    notification.on("close", cleanup);
    notification.show();
  }

  handle("attention-notify", (notice) => notify(notice));
  handle("attention-badge", (count, working) => setBadge(count, working));
  handle("app-preferences", () => ({ ...preferences }));
  handle("app-preferences-set", (patch) => setPreferences(patch));

  return {
    init,
    showWindow,
    /** Called from the window's `close` listener; returns true when the
     * event was intercepted (hidden) so main.cjs can `preventDefault()`. */
    handleWindowClose(win) {
      if (
        closeAction({ quitting, runInMenuBar: preferences.runInMenuBar }) !==
        "hide"
      )
        return false;
      if (win.isFullScreen()) {
        win.once("leave-full-screen", () => win.hide());
        win.setFullScreen(false);
      } else {
        win.hide();
      }
      return true;
    },
    setQuitting(value) {
      quitting = value;
    },
    close() {
      destroyTray();
      for (const notification of notifications) notification.close();
      notifications.clear();
    },
  };
}

module.exports = {
  DEFAULT_PREFERENCES,
  normalizePreferences,
  closeAction,
  trayTitle,
  trayState,
  validateNotice,
  registerAttentionIpc,
};
