// UI evidence driver for the routine BUILDER chat. The real app, real daemon
// and the real builder path (builder.start / reply / turn, session.read,
// parseTurn) run; only the model is fake: a tiny `claude` and `codex` in a temp
// dir at the front of PATH print the canned result the driver queued for the
// next turn. The routine list, show and grant calls are answered by a stub in
// the main process (the built routine, with access not yet granted).
//
//   npm run build && node .agents/skills/ui-evidence/scripts/builder.mjs
//
// States (each shot at 1440, 900 and 600 wide, files <state>-<width>.png under
// artifacts/routines-mvp/screens-v4/):
//   a-ask         Russian prose + owner card (choice, choice+other, text, multi)
//   a2-filled     the same card, answered
//   b-needs-you   run-result style JSON reply, shown readable
//   c-long        long Russian message with a markdown list
//   d-access      built routine with ungranted access: notice and sheet
//   e-failed      failed turn
//   f-no-sushiai  "sushiai is not available in this session" (codex)
// The real home is never touched: the profile is a short /tmp path and HOME,
// SUSHIAI_HOME, CODEX_HOME and the app data dir live in it, and the test window mode uses the mock
// keychain. The run exits 1 on sideways overflow, a page error or a failed step.
import { _electron as electron } from "playwright";
import { ELECTRON_ARGS } from "../../../../scripts/lib/browser.mjs";
import fs from "node:fs/promises";
import {
  stopDaemon,
  daemonBinary,
} from "../../../../scripts/lib/daemon-binary.mjs";

const root = process.cwd();
const outDir = `${root}/artifacts/routines-mvp/screens-v4`;
await fs.mkdir(outDir, { recursive: true });
const profile = await fs.mkdtemp("/tmp/sbld-");
const bin = `${profile}/bin`;
await fs.mkdir(bin, { recursive: true });

// ---- fake model ------------------------------------------------------------
// One script, two copies. It waits briefly for the prompt on stdin, then
// prints the queued turn in the format of its own name.
const FAKE = `#!${process.execPath}
const fs = require("node:fs");
const path = require("node:path");
const name = path.basename(process.argv[1]);
const args = process.argv.slice(2);
if (args.includes("--version")) {
  console.log("2.0.0 (fake " + name + ")");
  process.exit(0);
}
const turn = JSON.parse(fs.readFileSync(path.join(__dirname, "turn.json"), "utf8"));
const out = () => {
  if (name === "codex") {
    console.log(JSON.stringify({ type: "thread.started", thread_id: "fake-thread" }));
    if (turn.error) {
      console.log(JSON.stringify({ type: "turn.failed", error: { message: turn.text } }));
    } else {
      console.log(JSON.stringify({ type: "item.completed", item: { type: "agent_message", text: turn.text } }));
      console.log(JSON.stringify({ type: "turn.completed" }));
    }
  } else {
    console.log(JSON.stringify({ type: "result", subtype: turn.error ? "error" : "success", is_error: !!turn.error, result: turn.text, session_id: "fake-agent-session" }));
  }
  process.exit(0);
};
let done = false;
const finish = () => { if (!done) { done = true; setTimeout(out, 150); } };
process.stdin.on("data", finish);
process.stdin.on("end", finish);
process.stdin.on("error", finish);
setTimeout(finish, 900);
`;
for (const name of ["claude", "codex"])
  await fs.writeFile(`${bin}/${name}`, FAKE, { mode: 0o755 });
const queue = (text, error = false) =>
  fs.writeFile(`${bin}/turn.json`, JSON.stringify({ text, error }));

// ---- scripted turns (synthetic names only) ----------------------------------
const ASK = {
  title: "Нужно уточнить несколько деталей",
  questions: [
    {
      id: "project",
      type: "choice",
      label: "Для какого проекта собрать заметки?",
      options: [
        { value: "my-app", label: "my-app" },
        { value: "docs-site", label: "docs-site" },
      ],
    },
    {
      id: "when",
      type: "choice",
      label: "Когда запускать рутину?",
      allowOther: true,
      options: [
        { value: "fri", label: "По пятницам в 16:00" },
        { value: "weekday", label: "Каждое утро по будням" },
        { value: "night", label: "Каждую ночь" },
      ],
    },
    {
      id: "first",
      type: "text",
      label: "С чего должно начинаться сообщение?",
      placeholder: "Например: Привет, команда!",
    },
    {
      id: "where",
      type: "multi",
      label: "Куда отправить результат?",
      options: [
        { value: "team", label: "Канал команды" },
        { value: "mail", label: "Письмо на почту" },
        { value: "pr", label: "Комментарий к pull request" },
      ],
    },
  ],
  actions: [
    {
      id: "defaults",
      label: "Взять значения по умолчанию",
      kind: "reply",
      reply: "Используй значения по умолчанию",
      style: "secondary",
    },
  ],
};
const T = {
  ask:
    "Привет! Я прочитал описание и могу собрать рутину, но сначала нужно уточнить несколько деталей, чтобы она работала так, как вы ожидаете.\n\n```ask\n" +
    JSON.stringify(ASK, null, 2) +
    "\n```",
  needsYou: JSON.stringify({
    status: "needs_you",
    title: "Черновик готов, нужен ваш ответ",
    summary:
      "Я собрал черновик рутины для my-app. Осталось подтвердить время запуска и канал для результата.",
    next: "Ответьте на вопрос, и я сохраню рутину.",
  }),
  long:
    "Вот что я предлагаю сделать, чтобы рутина была надёжной и не создавала лишнего шума в канале команды. Сначала я читаю все слитые pull request за неделю, затем группирую их по теме и только после этого собираю итоговое сообщение для команды.\n\n" +
    "Что будет в сообщении:\n\n" +
    "- Короткая сводка по слитым изменениям: сколько pull request закрыто и какие из них самые важные для пользователей.\n" +
    "- Список задач, которые остались открытыми, с кратким пояснением, почему они ещё не закрыты.\n" +
    "- Очень длинный технический идентификатор ветки: my-app-feature-branch-with-a-really-long-name-that-should-wrap-and-not-push-the-page-wider-0123456789\n" +
    "- Ссылка на релизную страницу my-app, если релиз уже подготовлен.\n\n" +
    "1. Прочитать слитые pull request.\n2. Сгруппировать их по теме.\n3. Собрать черновик и показать его вам перед отправкой.\n\n" +
    "Если всё выглядит правильно, напишите «сохраняй», и я сохраню рутину как черновик.",
  saved:
    "Готово: рутина «Release notes» сохранена как черновик. Она запускается по пятницам в 16:00 на этом Mac. Чтобы она могла читать pull request, ей нужен доступ к инструментам — проверьте его справа.",
  failed: "The builder stopped: the model returned an error (synthetic).",
  noSushiai:
    "Не получилось сохранить рутину: sushiai is not available in this session. Проверьте, что команда sushiai установлена и доступна агенту, и повторите попытку.",
};

// The test app never registers modules in the owner's Claude or Codex files, so
// the throwaway daemon home carries the flag that makes the daemon serve them.
await fs.mkdir(`${profile}/h/modules`, { recursive: true, mode: 0o700 });
await fs.writeFile(`${profile}/h/modules/routines.enabled`, "");

// ---- app ---------------------------------------------------------------------
const app = await electron.launch({
  args: ELECTRON_ARGS,
  cwd: root,
  env: {
    ...process.env,
    HOME: profile, // the profile is the only home the app, daemon and fake CLIs see
    PATH: `${bin}:${process.env.PATH}`,
    SUSHIAI_TEST_WINDOW: "hidden",
    BRIDGE_DATA_DIR: `${profile}/data`,
    CODEX_HOME: `${profile}/codex`,
    SUSHIAI_HOME: `${profile}/h`,
    SUSHIAI_DAEMON_BIN: daemonBinary(root),
    BRIDGE_DEV_URL: "",
  },
});
const report = {
  pageErrors: [],
  shots: [],
  overflows: [],
  smallTargets: [],
  notes: [],
};
const shot = (name) => `${outDir}/${name}.png`;

// ---- fixtures for the stubbed list/show/grant calls ------------------------
const agent = {
  harness: "claude",
  model: "sonnet",
  effort: "medium",
  extraArgs: [],
};
const access = [
  {
    tool: "gh pr list",
    label: "Read pull requests of my-app",
    detail: "Lists merged pull requests; changes nothing.",
    kind: "read",
    granted: false,
  },
  {
    tool: "gh pr comment",
    label: "Comment on a pull request",
    detail: "Posts the draft as a comment.",
    kind: "write",
    granted: false,
  },
];
const BUILT = {
  routine: {
    id: "release-notes",
    name: "Release notes",
    where: { host: "my-mac", project: "my-app" },
    when: { cron: "0 16 * * 5", tz: "Europe/Berlin" },
    agent,
    howItRuns: { mode: "ask-first", promoteAfter: 5, accepted: 0 },
    limits: { timeoutMin: 30 },
    access,
    enabled: false,
  },
  lastRun: null,
  running: false,
  consecutiveFailures: 0,
  host: "local",
  hostName: "This Mac",
};
const BRIEF =
  "# Release notes\n\n## Goal\nDraft release notes for my-app.\n\n## Steps\n1. Read the merged pull requests of my-app.\n2. Draft the notes.\n\n## Result\nA draft on the release page.\n";
const CATALOG = {
  hosts: [
    {
      id: "local",
      name: "This Mac",
      state: "ready",
      routines: true,
      hostName: "my-mac",
      harnesses: {
        claude: {
          installed: true,
          loggedIn: true,
          models: [{ id: "sonnet", label: "Sonnet" }],
        },
        codex: {
          installed: true,
          loggedIn: true,
          models: [{ id: "gpt-local", label: "GPT Local" }],
        },
      },
    },
  ],
  projects: [
    {
      id: "my-app",
      name: "my-app",
      purpose: "The demo app",
      remote: "",
      folders: [{ host: "local", path: "/work/my-app" }],
    },
  ],
  defaults: { agent: { harness: "claude", model: "", effort: "medium" } },
};

// The real module answers builder.* and settings.*; everything else is stubbed.
const stubbed = await app.evaluate(
  ({ ipcMain }, { catalog, built, brief }) => {
    const original = ipcMain._invokeHandlers?.get("module:invoke");
    if (!original) return false;
    const st = (globalThis.__bld = { entries: [] });
    const clone = (v) => JSON.parse(JSON.stringify(v));
    const methods = {
      catalog: () => catalog,
      list: () => ({ routines: clone(st.entries), unreachable: [] }),
      show: () => ({
        routine: clone(st.entries[0]?.routine ?? built.routine),
        brief,
        hash: "h1",
        runs: [],
        consecutiveFailures: 0,
        latest: null,
      }),
      runs: () => ({ runs: [] }),
      validate: () => ({ ok: true, errors: [], warnings: [], hash: "h1" }),
      save: (_h, p) => ({ id: p.routine.id, hash: "h2", enabled: false }),
      grant: (_h, _id, tools) => {
        for (const e of st.entries)
          for (const a of e.routine.access ?? [])
            if (tools === "all" || tools.includes(a.tool)) a.granted = true;
        return { id: "release-notes", needsAccess: 0 };
      },
    };
    ipcMain.removeHandler("module:invoke");
    ipcMain.handle("module:invoke", (event, moduleId, method, ...args) =>
      moduleId === "builtin.routines" && method in methods
        ? methods[method](...args)
        : original(event, moduleId, method, ...args),
    );
    return true;
  },
  { catalog: CATALOG, built: BUILT, brief: BRIEF },
);
if (!stubbed) {
  console.log("could not wrap module:invoke (ipcMain._invokeHandlers missing)");
  await app.close();
  stopDaemon(`${profile}/h`);
  process.exit(1);
}
const setEntries = (entries) =>
  app.evaluate(({ BrowserWindow }, list) => {
    globalThis.__bld.entries = list;
    BrowserWindow.getAllWindows()[0].webContents.send("module-event", {
      moduleId: "builtin.routines",
      host: "local",
      event: { kind: "changed", routineId: "x", inbox: false },
    });
  }, entries);

// ---- measuring and shooting -------------------------------------------------
const WIDTHS = [1440, 900, 600];
async function measure(page, state, width) {
  const v = await page.evaluate(() => {
    const vw = document.documentElement.clientWidth;
    const rect = (el) => el.getBoundingClientRect();
    const sel = (el) =>
      el.tagName.toLowerCase() +
      (el.className && typeof el.className === "string"
        ? "." + el.className.trim().split(/\s+/).slice(0, 2).join(".")
        : "");
    const visible = (el) => {
      const r = rect(el);
      return (
        r.width > 0 &&
        r.height > 0 &&
        getComputedStyle(el).visibility !== "hidden"
      );
    };
    const scope = document.querySelector(".rt-page") ?? document.body;
    const scrollers = [...scope.querySelectorAll("*")]
      .filter((el) => el.scrollWidth - el.clientWidth > 1)
      .filter(
        (el) => !["auto", "scroll"].includes(getComputedStyle(el).overflowX),
      )
      .map((el) => ({ sel: sel(el), over: el.scrollWidth - el.clientWidth }))
      .slice(0, 6);
    const outside = [
      ...scope.querySelectorAll(".rt-chat *, .rt-built *, .ui-sheet *"),
    ]
      .filter(visible)
      .filter((el) => rect(el).right > vw + 1)
      .map((el) => `${sel(el)} right ${Math.round(rect(el).right)} > ${vw}`)
      .slice(0, 6);
    const small = [
      ...document.querySelectorAll(
        ".rt-page :is(button, [role=radio], [role=checkbox], textarea, input), .ui-sheet :is(button, textarea, input)",
      ),
    ]
      .filter(visible)
      .map((el) => ({
        sel: sel(el),
        text: (el.textContent || el.getAttribute("aria-label") || "")
          .trim()
          .slice(0, 30),
        w: Math.round(rect(el).width),
        h: Math.round(rect(el).height),
      }))
      .filter((t) => t.h < 24 || t.w < 24)
      .slice(0, 8);
    return {
      doc: document.documentElement.scrollWidth - vw,
      body: document.body.scrollWidth - vw,
      scrollers,
      outside,
      small,
    };
  });
  const sideways = [
    v.doc > 0 && `document +${v.doc}`,
    v.body > 0 && `body +${v.body}`,
    ...v.scrollers.map((s) => `${s.sel} +${s.over}`),
    ...v.outside,
  ].filter(Boolean);
  if (sideways.length) report.overflows.push({ state, width, sideways });
  if (v.small.length)
    report.smallTargets.push({ state, width, items: v.small });
}

async function capture(page, state, each = async () => {}) {
  for (const width of WIDTHS) {
    await app.evaluate(
      ({ BrowserWindow }, [w, h]) =>
        BrowserWindow.getAllWindows()[0].setContentSize(w, h),
      [width, width === 600 ? 760 : 860],
    );
    await page.waitForTimeout(350);
    await each();
    const file = shot(`${state}-${width}`);
    await page.screenshot({ path: file });
    report.shots.push(file);
    await measure(page, state, width);
    if (width === 1440) {
      try {
        const { writeMeasure } = await import(
          `${root}/.agents/skills/ui-evidence/scripts/measure.mjs`
        );
        await writeMeasure(page, `builder-${state}`, ".rt-builder", root);
      } catch (error) {
        report.notes.push(`measure ${state}: ${error.message}`);
      }
    }
  }
  // Back to the design size for the next step.
  await app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0].setContentSize(1440, 860),
  );
  await page.waitForTimeout(250);
}

const replyCount = (page) => page.locator(".rt-reply").count();
async function nextReply(page, before, label) {
  await page
    .waitForFunction(
      (n) =>
        document.querySelectorAll(".rt-reply").length > n &&
        !document.querySelector(".rt-status"),
      before,
      { timeout: 45000 },
    )
    .catch(() => report.notes.push(`${label}: no reply in 45 s`));
  await page.waitForTimeout(300);
}
async function send(page, text) {
  const field = page.getByLabel("Message to the builder");
  await field.fill(text);
  await page.getByRole("button", { name: "Send", exact: true }).last().click();
}

try {
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");
  await app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0].setContentSize(1440, 860),
  );
  await page.waitForTimeout(400);

  // Routines is on from the start; enable it only when a build left it off.
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  const enable = page.getByRole("button", { name: "Enable Routines" });
  if (await enable.count()) {
    await enable.click();
    await page.getByRole("button", { name: "Disable Routines" }).waitFor();
  }
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  await page.waitForTimeout(400);
  await page
    .locator(".sidebar")
    .getByRole("button", { name: "Routines", exact: true })
    .first()
    .click();
  await page.locator(".rt-start").waitFor();

  // a: first turn, the owner card.
  await queue(T.ask);
  await page
    .getByLabel("Describe the routine")
    .fill(
      "Каждую пятницу собирай заметки к релизу my-app из слитых pull request.",
    );
  await page.getByRole("button", { name: "Build", exact: true }).click();
  await page.locator(".rt-build").waitFor();
  await nextReply(page, 0, "a");
  await capture(page, "a-ask");

  // a2: the card answered (not sent yet).
  const card = page.locator(".rt-ask").last();
  await card.getByRole("radio", { name: "my-app", exact: true }).click();
  await card.getByRole("radio", { name: "Other…" }).click();
  await page
    .getByLabel("Когда запускать рутину?: other")
    .fill("Раз в две недели по вторникам в 10:30");
  await card
    .getByRole("textbox", { name: "С чего должно начинаться сообщение?" })
    .fill("Привет, команда! Вот что вышло на этой неделе.");
  await card.getByRole("button", { name: "Канал команды" }).click();
  await card
    .getByRole("button", { name: "Комментарий к pull request" })
    .click();
  await capture(page, "a2-filled");

  // b: answers sent, the reply is run-result JSON.
  await queue(T.needsYou);
  let n = await replyCount(page);
  await card.getByRole("button", { name: "Send answers" }).click();
  await nextReply(page, n, "b");
  await capture(page, "b-needs-you");

  // c: a long message with a list.
  await queue(T.long);
  n = await replyCount(page);
  await send(page, "Расскажи подробнее, что именно будет в сообщении.");
  await nextReply(page, n, "c");
  await capture(page, "c-long");

  // d: the routine is saved; its access waits.
  await queue(T.saved);
  n = await replyCount(page);
  await setEntries([BUILT]);
  await send(page, "Сохраняй.");
  await nextReply(page, n, "d");
  await page
    .getByRole("dialog")
    .first()
    .waitFor({ timeout: 8000 })
    .catch(() => report.notes.push("d: access sheet did not open"));
  // The sheet opens where the owner is: reopen it after each resize.
  const sheet = page.getByRole("dialog", { name: /needs access/ });
  await capture(page, "d-access-sheet", async () => {
    if (await sheet.count()) await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Review access" }).click();
    await sheet.waitFor();
    await page.waitForTimeout(250);
  });
  const cancel = page.getByRole("button", { name: "Cancel", exact: true });
  if (await cancel.count()) await cancel.first().click();
  await page.waitForTimeout(300);
  await capture(page, "d-access-notice");

  // e: a failed turn.
  await queue(T.failed, true);
  n = await replyCount(page);
  await send(page, "Добавь ещё один шаг.");
  await nextReply(page, n, "e");
  await capture(page, "e-failed");

  // f: codex says sushiai is missing.
  await setEntries([]);
  await page
    .locator(".rt-builder")
    .getByRole("button", { name: "Routines", exact: true })
    .click();
  await page.locator(".rt-start").waitFor();
  await page.evaluate(() =>
    window.bridge.moduleInvoke("builtin.routines", "settings.set", {
      harness: "codex",
      effort: "medium",
    }),
  );
  await queue(T.noSushiai);
  await page
    .getByLabel("Describe the routine")
    .fill("Каждый день проверяй сборку my-app и пиши в канал команды.");
  await page.getByRole("button", { name: "Build", exact: true }).click();
  await page.locator(".rt-build").waitFor();
  await nextReply(page, 0, "f");
  await capture(page, "f-no-sushiai");
} catch (error) {
  report.error = String(error?.stack ?? error);
  try {
    await (
      await app.firstWindow()
    ).screenshot({ path: shot("ERR-last-state") });
  } catch {}
} finally {
  await app.close();
  stopDaemon(`${profile}/h`);
  await fs.rm(profile, {
    recursive: true,
    force: true,
    maxRetries: 5,
    retryDelay: 200,
  });
  console.log(JSON.stringify(report, null, 2));
  if (report.error || report.overflows.length || report.pageErrors.length)
    process.exitCode = 1;
}
