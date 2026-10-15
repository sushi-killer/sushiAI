// UI evidence driver for the built-in Routines extension (R1-R11). The daemon
// module is not running here: the main process answers `module:invoke` for
// builtin.routines and the few daemon session channels with synthetic
// fixtures (my-app, build-host). Run from the repo root after `npm run build`.
//
// It walks every state once at the design size (the Figma parity pass, files
// <id>-<name>.png and artifacts/routines-<id>.measure.json), then again at
// 1440, 1100, 900 and the app's minimum window width (files
// w<width>-<id>-<name>.png). Every state asserts that the document does not
// scroll sideways; the run exits 1 when one does.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { stopDaemon } from "../../../../scripts/lib/daemon-binary.mjs";

const root = process.cwd();
const outDir = `${root}/artifacts/routines-mvp/screens-v3`;
await fs.mkdir(outDir, { recursive: true });
const shot = (name) => `${outDir}/${name}.png`;
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
const app = await electron.launch({
  args: ["."],
  cwd: root,
  env: {
    ...process.env,
    SUSHIAI_TEST_WINDOW: "hidden",
    BRIDGE_DATA_DIR: profile,
    HOME: profile,
    CODEX_HOME: `${profile}/codex`,
    SUSHIAI_HOME: `${profile}/sushiai`,
    BRIDGE_DEV_URL: "",
  },
});
const report = { pageErrors: [], shots: {}, measures: {}, notes: [] };

// ---- fixtures -------------------------------------------------------------
const now = Date.now();
const H = 3600e3;
const agent = {
  harness: "claude",
  model: "sonnet",
  effort: "medium",
  extraArgs: [],
};
const routine = (id, name, over = {}) => ({
  id,
  name,
  where: { host: "my-mac", project: "my-app" },
  when: { cron: "0 9 * * 1-5", tz: "Europe/Berlin" },
  agent,
  howItRuns: { mode: "ask-first", promoteAfter: 5, accepted: 2 },
  limits: { timeoutMin: 30 },
  enabled: true,
  ...over,
});
const run = (n, status, over = {}) => ({
  n,
  kind: "scheduled",
  status,
  startedAt: now - 3 * H,
  endedAt: now - 3 * H + 120e3,
  agent: { harness: "claude", model: "sonnet", effort: "medium" },
  ...over,
});
const LONG_SUMMARY = [
  "# Weekly report for my-app",
  "",
  "Seven pull requests merged, two issues closed, one release candidate cut.",
  "",
  "## What changed",
  "- Scheduler now keeps the last ten runs per routine.",
  "- The settings tab got a builder host picker.",
  "- A very long unbroken identifier: my-app-feature-branch-with-a-really-long-name-that-should-wrap-or-scroll-and-not-push-the-page-wider-than-the-window-0123456789",
  "",
  "## Still open",
  "1. Decide on the retention of run logs.",
  "2. Review the build-host runner image.",
].join("\n");
const ACTIONS = [
  {
    kind: "issue",
    text: "Labelled 4 new issues of my-app as bug",
    applied: true,
    link: "https://example.com/my-app/issues",
  },
  {
    kind: "comment",
    text: "Commented on PR 12 with the release checklist",
    applied: true,
  },
  {
    kind: "branch",
    text: "Delete 3 merged branches of my-app",
    applied: false,
  },
  {
    kind: "issue",
    text: "Close 2 stale issues older than 90 days",
    applied: false,
    link: "https://example.com/my-app/issues/stale",
  },
];
const resultOf = (status, title, summary, over = {}) => ({
  status,
  title,
  summary,
  actions: [],
  ...over,
});

const E = {
  weekly: {
    routine: routine("weekly-report", "Report to team", {
      when: { cron: "0 17 * * 5", tz: "Europe/Berlin" },
    }),
    lastRun: run(7, "needs_you", {
      title: "Draft ready for the team channel",
    }),
    running: false,
    consecutiveFailures: 0,
    host: "local",
    hostName: "This Mac",
  },
  intake: {
    routine: routine("issue-intake", "Triage issues"),
    lastRun: run(5, "needs_you", { title: "3 issues need a decision" }),
    running: false,
    consecutiveFailures: 0,
    host: "local",
    hostName: "This Mac",
  },
  watch: {
    routine: routine("build-watch", "Build watch", {
      where: { host: "build-host", project: "my-app" },
      when: { cron: "*/30 * * * *", tz: "UTC" },
      howItRuns: { mode: "on-its-own", promoteAfter: 5, accepted: 5 },
    }),
    lastRun: run(9, "failed", {
      title: "Build check failed",
      reason:
        "The gh tool is not signed in on build-host. Run `gh auth login` there, then run the routine again.",
      loginHost: "build-host",
      loginHarness: "claude",
    }),
    running: false,
    consecutiveFailures: 1,
    host: "ssh:build-host",
    hostName: "build-host",
  },
  stale: {
    routine: routine("stale-pr-digest", "Stale PR digest", {
      when: { cron: "0 18 * * *", tz: "Europe/Berlin" },
    }),
    lastRun: run(8, "done", { title: "Four pull requests are stale" }),
    running: false,
    consecutiveFailures: 0,
    host: "local",
    hostName: "This Mac",
  },
  digest: {
    routine: routine("morning-digest", "Morning digest", {
      when: { cron: "30 8 * * 1-5", tz: "Europe/Berlin" },
    }),
    lastRun: null,
    running: false,
    consecutiveFailures: 0,
    host: "local",
    hostName: "This Mac",
  },
  cleanup: {
    routine: routine("branch-cleanup", "Branch cleanup", {
      enabled: false,
      when: { cron: "0 17 * * 5", tz: "Europe/Berlin" },
    }),
    lastRun: null,
    running: false,
    consecutiveFailures: 0,
    host: "local",
    hostName: "This Mac",
  },
};
const BUILT = {
  routine: routine("release-notes", "Release notes", {
    enabled: false,
    when: { cron: "0 16 * * 5", tz: "Europe/Berlin" },
  }),
  lastRun: null,
  running: false,
  consecutiveFailures: 0,
  host: "local",
  hostName: "This Mac",
};

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
          models: [
            { id: "sonnet", label: "Sonnet" },
            { id: "opus", label: "Opus" },
          ],
        },
        codex: {
          installed: true,
          loggedIn: true,
          models: [{ id: "gpt-local", label: "GPT Local" }],
        },
      },
    },
    {
      id: "ssh:build-host",
      name: "build-host",
      state: "ready",
      routines: true,
      hostName: "build-host",
      harnesses: {
        claude: {
          installed: true,
          loggedIn: false,
          models: [{ id: "sonnet", label: "Sonnet" }],
        },
        codex: { installed: false, loggedIn: null, models: [] },
      },
    },
  ],
  projects: [
    {
      id: "my-app",
      name: "my-app",
      purpose: "The demo app",
      remote: "",
      folders: [
        { host: "local", path: "/work/my-app" },
        { host: "ssh:build-host", path: "/srv/my-app" },
      ],
    },
  ],
  defaults: { agent: { harness: "claude", model: "", effort: "medium" } },
};

const detailOf = (
  entry,
  runs,
  latest,
  brief = "# Report\n\n## Goal\nWrite the weekly report of my-app.\n\n## Steps\n1. Read the merged pull requests of my-app.\n2. Draft the message.\n\n## Result\nPost the draft to the team channel.\n",
) => ({
  routine: entry.routine,
  brief,
  hash: "h1",
  runs,
  consecutiveFailures: entry.consecutiveFailures,
  latest,
});
const RELEASE_BRIEF =
  "# Release notes\n\n## Goal\nDraft release notes for my-app.\n\n## Steps\n1. Read the merged pull requests of my-app.\n2. Draft the notes.\n\n## Result\nA draft on the release page.\n";
const key = (entry) => `${entry.host}:${entry.routine.id}`;
const summ = (entry) => entry.lastRun;

// Base state: four routines of one list.
function listState({ weeklyRuns, intakeRuns, watchRuns, extra = {} } = {}) {
  const entries = [E.weekly, E.intake, E.watch, E.stale, E.digest, E.cleanup];
  const weeklyResult = resultOf(
    "needs_you",
    "Draft ready for the team channel",
    "Shipped: onboarding v2, push fixes. In review: paywall copy.",
  );
  const staleResult = resultOf(
    "done",
    "Four pull requests are stale",
    LONG_SUMMARY,
    { actions: ACTIONS, next: "Next run today at 18:00." },
  );
  const intakeResult = resultOf(
    "needs_you",
    "3 issues need a decision",
    "Three issues of my-app need a decision from you.",
    { next: "Answer, then run again." },
  );
  const watchResult = resultOf(
    "failed",
    "Build check failed",
    "The build status of my-app could not be read.",
    { missing: ["gh"] },
  );
  const details = {
    [key(E.weekly)]: detailOf(
      E.weekly,
      weeklyRuns ?? [
        E.weekly.lastRun,
        run(6, "noop", {
          title: "Nothing to do",
          startedAt: now - 27 * H,
          endedAt: now - 27 * H + 30e3,
        }),
        run(4, "done", {
          kind: "dry",
          title: "Dry run",
          startedAt: now - 50 * H,
          endedAt: now - 50 * H + 60e3,
        }),
      ],
      weeklyResult,
    ),
    [key(E.intake)]: detailOf(
      E.intake,
      intakeRuns ?? [E.intake.lastRun],
      intakeResult,
    ),
    [key(E.watch)]: detailOf(
      E.watch,
      watchRuns ?? [E.watch.lastRun],
      watchResult,
    ),
    [key(E.stale)]: detailOf(E.stale, [E.stale.lastRun], staleResult),
    [key(E.digest)]: detailOf(E.digest, [], null),
    [key(E.cleanup)]: detailOf(E.cleanup, [], null),
  };
  const results = {
    [`${key(E.weekly)}:7`]: { run: E.weekly.lastRun, result: weeklyResult },
    [`${key(E.weekly)}:6`]: {
      run: details[key(E.weekly)].runs[1],
      result: resultOf("noop", "Nothing to do", "No change since last week."),
    },
    [`${key(E.intake)}:5`]: { run: E.intake.lastRun, result: intakeResult },
    [`${key(E.stale)}:8`]: { run: E.stale.lastRun, result: staleResult },
    [`${key(E.watch)}:9`]: { run: E.watch.lastRun, result: watchResult },
  };
  return { entries, details, results, ...extra };
}

// ---- main-process stub ------------------------------------------------------
await app.evaluate(({ ipcMain, BrowserWindow }, catalog) => {
  const rt = (globalThis.__rt = {
    state: {
      entries: [],
      details: {},
      results: {},
      sessions: [],
      screens: {},
      hooks: {},
    },
    catalog,
    settings: {
      harness: "claude",
      model: "",
      effort: "medium",
      host: "local",
      accountId: "",
    },
    calls: [],
    args: [],
  });
  const S = () => rt.state;
  const clone = (v) => JSON.parse(JSON.stringify(v));
  const methods = {
    catalog: () => rt.catalog,
    list: () => ({ routines: clone(S().entries), unreachable: [] }),
    show: (host, id) => clone(S().details[`${host}:${id}`]),
    runs: (host, id) => ({ runs: S().details[`${host}:${id}`]?.runs ?? [] }),
    result: (host, id, run) => {
      const d = S().details[`${host}:${id}`];
      const n = run ?? d?.runs?.[0]?.n;
      const hit = S().results[`${host}:${id}:${n}`];
      if (hit) return clone(hit);
      const summary = d?.runs?.find((r) => r.n === n);
      return clone({ run: summary, result: null });
    },
    validate: () => ({ ok: true, errors: [], warnings: [], hash: "h1" }),
    save: (host, payload) => ({
      id: payload.routine.id,
      hash: "h2",
      enabled: payload.routine.enabled,
    }),
    setEnabled: (host, id, enabled) => ({ id, enabled }),
    runNow: () => ({ session: "sess-run" }),
    dryRun: () => {
      const hook = S().hooks.dryRun;
      if (hook) Object.assign(S(), clone(hook));
      return { run: 1, session: "sess-dry" };
    },
    delete: (host, id) => ({ id }),
    move: (from, to, id) => ({ host: to, id }),
    "settings.get": () => rt.settings,
    "settings.set": (patch) => Object.assign(rt.settings, patch),
    grant: (host, id, tools) => {
      const entry = S().entries.find(
        (item) => `${item.host}:${item.routine.id}` === `${host}:${id}`,
      );
      for (const item of entry?.routine.access ?? [])
        if (tools === "all" || tools.includes(item.tool)) item.granted = true;
      const detail = S().details[`${host}:${id}`];
      if (detail && entry) detail.routine = clone(entry.routine);
      return { id, needsAccess: 0 };
    },
    approve: (host, id, run, proposal) => ({
      id,
      run,
      proposal,
      status: "applied",
    }),
    "open-login-terminal": () => ({
      host: "ssh:build-host",
      sessionHost: "build-host",
      sessionId: "sess-login",
    }),
    "builder.start": () => {
      const hook = S().hooks.builder;
      if (hook) Object.assign(S(), clone(hook));
      return { host: "local", sessionHost: "local", sessionId: "sess-build" };
    },
  };
  ipcMain.removeHandler("module:invoke");
  ipcMain.handle("module:invoke", async (_event, moduleId, method, ...args) => {
    rt.calls.push(`${moduleId}.${method}`);
    rt.args.push([method, args]);
    if (moduleId !== "builtin.routines" || !(method in methods))
      throw new Error(`stub: ${moduleId}.${method}`);
    return methods[method](...args);
  });
  ipcMain.removeHandler("daemon-sessions-list");
  ipcMain.handle("daemon-sessions-list", async () => clone(S().sessions));
  ipcMain.removeHandler("daemon-session-read");
  ipcMain.handle("daemon-session-read", async (_e, _host, id) => ({
    text: S().screens[id] ?? "",
  }));
  ipcMain.removeHandler("daemon-session-input");
  ipcMain.handle("daemon-session-input", async () => true);
  ipcMain.removeHandler("daemon-ask-respond");
  ipcMain.handle("daemon-ask-respond", async () => true);
  // Accounts for Settings -> Routines.
  for (const channel of ["claude-accounts-list", "codex-accounts-list"]) {
    ipcMain.removeHandler(channel);
    ipcMain.handle(channel, async () => [
      { id: "acct-1", label: "Work account" },
    ]);
  }
  rt.emit = () => {
    const win = BrowserWindow.getAllWindows()[0];
    win.webContents.send("module-event", {
      moduleId: "builtin.routines",
      host: "local",
      event: { kind: "changed", routineId: "x", inbox: false },
    });
  };
}, CATALOG);

const put = (state) =>
  app.evaluate((_e, s) => {
    Object.assign(globalThis.__rt.state, s);
    globalThis.__rt.emit();
  }, state);

// ---- measurement ----------------------------------------------------------
const TOKENS = (
  await Promise.all(
    ["src/styles/components/routines.css", "src/ui/ui.css"].map((file) =>
      fs.readFile(`${root}/${file}`, "utf8"),
    ),
  )
)
  .flatMap((text) => text.match(/var\((--[a-zA-Z0-9-]+)/g) ?? [])
  .map((s) => s.slice(4));
const TOKEN_SET = [...new Set(TOKENS)];

/** The app's smallest window (electron/main.cjs `minWidth`). */
const MIN_WIDTH = Number(
  (await fs.readFile(`${root}/electron/main.cjs`, "utf8")).match(
    /minWidth:\s*(\d+)/,
  )?.[1] ?? 600,
);
const MIN_HEIGHT = Number(
  (await fs.readFile(`${root}/electron/main.cjs`, "utf8")).match(
    /minHeight:\s*(\d+)/,
  )?.[1] ?? 440,
);
const WIDTHS = [1440, 1100, 900, MIN_WIDTH];
report.minWidth = MIN_WIDTH;
report.overflows = [];

async function measure(page, name) {
  const value = await page.evaluate((tokens) => {
    const q = (s) => [...document.querySelectorAll(s)];
    const vw = document.documentElement.clientWidth;
    const scrollers = [
      ".section-page",
      ".rt-page",
      ".rt-split",
      ".rt-list",
      ".rt-pane",
      ".rt-detail",
      ".rt-run",
      ".rt-build",
      ".rt-chat",
      ".rt-built",
      ".rt-result",
      ".rt-start",
      ".rt-settings",
      ".ui-doc",
      ".ui-sheet",
      ".ui-composer",
    ]
      .flatMap((sel) =>
        q(sel).map((el) => ({
          sel,
          over: el.scrollWidth - el.clientWidth,
          w: Math.round(el.getBoundingClientRect().width),
        })),
      )
      .filter((m) => m.over > 0);
    // Clipped text: ellipsis or hidden overflow inside the routines page.
    const root = document.querySelector(".rt-page, .rt-settings");
    const clipped = root
      ? [...root.querySelectorAll("*")]
          .filter((el) => {
            if (
              !el.textContent?.trim() ||
              (el.children.length > 0 && el.tagName !== "BUTTON")
            )
              return false;
            const cs = getComputedStyle(el);
            const clips =
              cs.overflow !== "visible" || cs.textOverflow === "ellipsis";
            return clips && el.scrollWidth > el.clientWidth + 1;
          })
          .map((el) => ({
            tag: el.tagName,
            cls: el.className,
            text: el.textContent.trim().slice(0, 60),
            scroll: el.scrollWidth,
            client: el.clientWidth,
          }))
      : [];
    const zero = root
      ? [...root.querySelectorAll(".ui-kv-label, button, .ui-tag, h2, h3")]
          .filter(
            (el) =>
              el.textContent?.trim() && el.getBoundingClientRect().width === 0,
          )
          .map((el) => el.className || el.tagName)
      : [];
    // A menu or sheet must sit inside the window.
    const floating = q(".ui-pop-panel, .ui-sheet")
      .map((el) => {
        const r = el.getBoundingClientRect();
        return {
          cls: el.className,
          left: Math.round(r.left),
          right: Math.round(r.right),
          top: Math.round(r.top),
          bottom: Math.round(r.bottom),
        };
      })
      .filter(
        (r) =>
          r.left < 0 ||
          r.right > vw + 0.5 ||
          r.top < 0 ||
          r.bottom > window.innerHeight + 0.5,
      );
    const cs = getComputedStyle(document.documentElement);
    const missingTokens = tokens.filter((t) => !cs.getPropertyValue(t).trim());
    return {
      viewportW: vw,
      docOverflow: document.documentElement.scrollWidth - vw,
      bodyOverflow: document.body.scrollWidth - vw,
      scrollers,
      clipped,
      zeroWidth: zero,
      floating,
      missingTokens,
      rightEdgeMax: root
        ? Math.max(
            ...[...root.querySelectorAll("*")].map((el) =>
              Math.round(el.getBoundingClientRect().right),
            ),
          )
        : null,
    };
  }, TOKEN_SET);
  report.measures[name] = value;
  // The assertion: nothing scrolls sideways, nothing floats off the window.
  const sideways = [
    value.docOverflow > 0 && `document +${value.docOverflow}`,
    value.bodyOverflow > 0 && `body +${value.bodyOverflow}`,
    ...value.scrollers.map((s) => `${s.sel} +${s.over}`),
    ...value.floating.map((f) => `${f.cls} outside the window`),
  ].filter(Boolean);
  if (sideways.length) report.overflows.push({ state: name, sideways });
}

let tag = ""; // "" at the design size, "w900-" in the width sweep
async function capture(
  page,
  id,
  name,
  { selector = ".workspace-canvas", parity = !tag } = {},
) {
  await page.waitForTimeout(450);
  await measure(page, `${tag}${id}`);
  if (parity) {
    const { writeMeasure } = await import(
      `${root}/.agents/skills/ui-evidence/scripts/measure.mjs`
    );
    await writeMeasure(page, `routines-${id}`, selector);
  }
  const path = shot(`${tag}${id}-${name}`);
  await page.screenshot({ path });
  report.shots[`${tag}${id}`] = path;
}

/** Picks a routine in the list; a narrow page shows the list only after the
 * back control of the detail. */
async function pick(page, name) {
  await page.locator(".rt-split").waitFor();
  const back = page.locator(".rt-to-list");
  if (await back.isVisible().catch(() => false)) await back.click();
  await page.locator(".ui-list-row", { hasText: name }).click();
  await page.waitForTimeout(250);
}

const sidebarRoutines = (page) =>
  page
    .locator(".sidebar")
    .getByRole("button", { name: "Routines", exact: true })
    .first();

/** One pass over every state. At the design size it is the parity pass. */
async function walk(page) {
  // R1: nothing yet.
  await put({
    entries: [],
    details: {},
    results: {},
    sessions: [],
    screens: {},
  });
  await page.waitForTimeout(300);
  await page.locator(".rt-start").waitFor();
  await capture(page, "R1", "start");

  // The composer grows with the text, stops at six lines and shrinks back.
  if (!tag) {
    const field = page.getByLabel("Describe the routine");
    const height = () =>
      field.evaluate((el) => el.getBoundingClientRect().height);
    const bottoms = () =>
      page.evaluate(() => {
        const form = document.querySelector(".ui-composer");
        const field = form.querySelector("textarea");
        const button = form.querySelector("button");
        return {
          field: Math.round(field.getBoundingClientRect().bottom),
          button: Math.round(button.getBoundingClientRect().bottom),
          fieldTop: Math.round(field.getBoundingClientRect().top),
          overflowY: getComputedStyle(field).overflowY,
        };
      });
    await field.fill("");
    const one = await height();
    await field.fill("a\nb\nc");
    const three = await height();
    const threeEdges = await bottoms();
    await field.fill(
      Array.from({ length: 12 }, (_, i) => `line ${i}`).join("\n"),
    );
    const twelve = await height();
    const twelveEdges = await bottoms();
    await field.fill("");
    const back = await height();
    report.composer = { one, three, twelve, back, threeEdges, twelveEdges };
    if (!(
      three > one &&
      twelve > three &&
      twelve <= 6 * 16 + 14 + 1 &&
      back === one
    ))
      report.overflows.push({
        state: "composer",
        sideways: [`heights ${one}/${three}/${twelve}/${back}`],
      });
    if (
      twelveEdges.overflowY !== "auto" ||
      twelveEdges.button !== twelveEdges.field
    )
      report.overflows.push({
        state: "composer",
        sideways: ["button not on the bottom edge or no scroll at six lines"],
      });
  }

  // R2: list + detail, the Where and Agent pickers.
  await put(listState());
  await pick(page, "Report to team");
  await page.locator(".ui-rows").first().waitFor();
  await page
    .locator(".ui-rows")
    .first()
    .getByRole("button", { name: "Agent" })
    .click();
  await page.getByRole("dialog", { name: "Agent" }).waitFor();
  report.pickers = {
    agentPopover: await page.getByRole("dialog", { name: "Agent" }).innerText(),
  };
  await capture(page, "R2", "list-detail-agent-picker");
  await page.keyboard.press("Escape");
  await page
    .locator(".ui-rows")
    .first()
    .getByRole("button", { name: "Where" })
    .click();
  report.pickers.wherePopover = await page
    .getByRole("dialog", { name: "Where" })
    .innerText();
  await measure(page, `${tag}R2-where`);
  await page.keyboard.press("Escape");
  // The build-host routine: its agent shows the not-logged-in problem.
  await pick(page, "Build watch");
  await page.waitForTimeout(400);
  await capture(page, "R2d", "build-host-detail");

  // R3: builder.
  const base = listState();
  const builtDetail = detailOf(BUILT, [], null, RELEASE_BRIEF);
  const dryRunOf = (extra = {}) =>
    run(1, "done", {
      kind: "dry",
      title: "Release notes for my-app",
      startedAt: now - 60e3,
      endedAt: now - 20e3,
      ...extra,
    });
  await put({
    sessions: [],
    screens: {
      "sess-build":
        "claude> Reading the repo of my-app...\nclaude> Saved routine release-notes (draft).\n\nThe routine runs Fridays at 16:00 on This Mac.\nRun a dry run to see what it would do.",
    },
    hooks: {
      builder: {
        entries: [...base.entries, BUILT],
        details: { ...base.details, [key(BUILT)]: builtDetail },
      },
      dryRun: {
        entries: [...base.entries, { ...BUILT, lastRun: dryRunOf() }],
        details: {
          ...base.details,
          [key(BUILT)]: detailOf(BUILT, [dryRunOf()], null, RELEASE_BRIEF),
        },
        results: {
          ...base.results,
          [`${key(BUILT)}:1`]: {
            run: dryRunOf(),
            result: resultOf(
              "done",
              "Release notes for my-app",
              "## Release notes\n\n- Scheduler keeps the last ten runs.\n- Builder host picker in Settings.\n\nThis was a dry run: nothing was written.",
              {
                actions: [
                  {
                    kind: "file",
                    text: "Write RELEASE_NOTES.md in my-app",
                    applied: false,
                  },
                  {
                    kind: "comment",
                    text: "Post the notes on PR 14",
                    applied: false,
                    link: "https://example.com/my-app/pull/14",
                  },
                ],
              },
            ),
          },
        },
      },
    },
  });
  const toList = page.locator(".rt-to-list");
  if (await toList.isVisible().catch(() => false)) await toList.click();
  await page.getByRole("button", { name: "New routine" }).click();
  await page
    .getByLabel("Describe the routine")
    .fill(
      "Every Friday at 16:00, draft release notes for my-app from the merged pull requests.",
    );
  await page.getByRole("button", { name: "Build", exact: true }).click();
  await page.locator(".rt-build").waitFor();
  await page.locator(".rt-built .ui-rows").waitFor();
  await page
    .locator(".rt-built .ui-rows")
    .getByRole("button", { name: "Where" })
    .click();
  await capture(page, "R3", "builder-where-open");
  await page.keyboard.press("Escape");

  // R4: dry run result.
  await page
    .locator(".rt-built")
    .getByRole("button", { name: "Dry run" })
    .click();
  await page.locator(".rt-built .rt-run").waitFor();
  await page.locator(".rt-built .rt-result .ui-rows").waitFor();
  await capture(page, "R4", "dry-run");

  // Back to the list: keep as draft.
  await page.getByRole("button", { name: "Save routine" }).click();
  await page.locator(".rt-split").waitFor();

  // R5: owner ask card on a live run + R7 live log.
  const runningRun = run(10, "running", {
    title: undefined,
    startedAt: now - 90e3,
    endedAt: undefined,
    session: "sess-run",
  });
  const intakeLive = { ...E.intake, lastRun: runningRun, running: true };
  const liveState = listState({
    intakeRuns: [
      runningRun,
      run(5, "needs_you", { title: "3 issues need a decision" }),
    ],
  });
  liveState.entries = [
    E.weekly,
    intakeLive,
    E.watch,
    E.stale,
    E.digest,
    E.cleanup,
  ];
  liveState.details[key(E.intake)] = detailOf(
    intakeLive,
    [runningRun, E.intake.lastRun],
    null,
  );
  liveState.details[key(E.intake)].latest = null;
  await put({
    ...liveState,
    sessions: [
      {
        id: "sess-run",
        asks: [
          {
            askId: "ask-1",
            session: "sess-run",
            tool: "owner.ask",
            input: {
              text: "Issue 41 of my-app looks like a duplicate of issue 12. Close it as a duplicate?",
              options: [
                { id: "close", label: "Close as duplicate" },
                { id: "keep", label: "Keep it open" },
              ],
            },
          },
        ],
      },
    ],
    screens: {
      "sess-run":
        '$ sushiai ask --text "Issue 41 of my-app looks like a duplicate..."\nwaiting for the owner...\n\n[09:00:12] read 14 open issues of my-app\n[09:00:31] grouped 3 by likely cause',
    },
  });
  await pick(page, "Triage issues");
  await page.locator(".rt-runs button").first().waitFor();
  await page.locator(".rt-runs button").first().click();
  await page.locator(".rt-review").waitFor();
  await capture(page, "R5", "review");

  // R7: live log, no question open.
  await put({
    sessions: [{ id: "sess-run", asks: [] }],
    screens: {
      "sess-run":
        "[09:00:12] read 14 open issues of my-app\n[09:00:31] grouped 3 by likely cause\n[09:00:48] drafting the list of decisions\n[09:01:02] writing result.json ...",
    },
  });
  await app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0].webContents.send("daemon-event", {
      host: "local",
      method: "session.askClosed",
      params: { askId: "ask-1" },
    }),
  );
  await page.waitForTimeout(2600); // the log polls every 2s
  await page
    .locator(".rt-review")
    .waitFor({ state: "detached", timeout: 5000 })
    .catch(() =>
      report.notes.push(
        "R7: ask card stayed after the ask list emptied (asks only refresh on daemon events)",
      ),
    );
  await capture(page, "R7", "run");

  // R5b: the changes a finished run waits to apply, one checkbox each.
  const proposals = [
    ["Fix crash on empty cart screen", "task"],
    ["Update privacy link in settings", "task"],
    ["Bump SDK to 4.2", "task"],
  ].map(([title, kind]) => ({
    status: "pending",
    applied: false,
    proposal: { kind, title },
  }));
  const approveRun = run(5, "needs_you", {
    title: "3 changes need your OK",
    proposals,
    pendingApprovals: 3,
  });
  const approveState = listState({ intakeRuns: [approveRun] });
  approveState.entries = approveState.entries.map((entry) =>
    entry === E.intake ? { ...E.intake, lastRun: approveRun } : entry,
  );
  await put({ ...approveState, sessions: [], screens: {} });
  await pick(page, "Triage issues");
  await page.locator(".rt-runs button").first().click();
  await page.locator('[aria-label="Changes waiting for your OK"]').waitFor();
  await capture(page, "R5b", "approve", { parity: false });
  await page.getByRole("button", { name: /^Create 3 tasks$/ }).waitFor();
  await page.getByRole("checkbox", { name: "Bump SDK to 4.2" }).uncheck();
  report.approveButton = await page
    .locator('[aria-label="Changes waiting for your OK"] .ui-button.primary')
    .innerText();

  // R6: a failed routine: the notice and the scheme, as in the design.
  await put({ ...listState(), sessions: [], screens: {} });
  await pick(page, "Build watch");
  await page.locator(".ui-notice.ui-tone-danger").waitFor();
  await capture(page, "R6", "failed");
  report.loginButton = await page
    .getByRole("button", { name: "Log in on build-host" })
    .count();
  // The run behind it: what failed and the command that fixes it.
  await page.locator(".rt-runs button").first().click();
  await page.locator(".rt-run [role=alert]").first().waitFor();
  await page.waitForTimeout(300);
  await measure(page, `${tag}R6b`);
  await page.screenshot({ path: shot(`${tag}R6b-failed-run`) });

  // R10: result with applied and would-do actions.
  await pick(page, "Stale PR digest");
  await page.locator(".rt-runs button").first().click();
  await page.locator(".rt-result .ui-rows").waitFor();
  await capture(page, "R10", "result");

  // R11: read as document.
  await page.getByRole("button", { name: "Open as doc" }).click();
  await page.locator(".ui-doc").waitFor();
  await capture(page, "R11", "result-doc");

  // R8: the tools a routine waits for.
  const access = [
    {
      tool: "mcp__asana__read",
      label: "Read tasks",
      detail: "Asana · Mobile",
      kind: "read",
      granted: false,
    },
    {
      tool: "mcp__asana__comment",
      label: "Comment on tasks",
      detail: "Asana · Mobile",
      kind: "write",
      granted: false,
    },
    {
      tool: "Bash(sushiai propose:*)",
      label: "Create tasks",
      detail: "orchd · app-repo",
      kind: "write",
      granted: false,
    },
  ];
  const needy = listState();
  needy.entries = needy.entries.map((entry) =>
    entry === E.intake
      ? { ...E.intake, routine: { ...E.intake.routine, access } }
      : entry,
  );
  needy.details[key(E.intake)] = {
    ...needy.details[key(E.intake)],
    routine: { ...E.intake.routine, access },
  };
  await put({ ...needy, sessions: [], screens: {} });
  const doc = page.locator(".ui-doc");
  if (await doc.count()) await page.locator(".ui-crumb-link").first().click();
  await page.locator(".rt-split").waitFor();
  await pick(page, "Triage issues");
  await page
    .getByRole("dialog", { name: "Triage issues needs access" })
    .waitFor();
  await capture(page, "R8", "access-dialog", { parity: false });
  await page.getByRole("button", { name: "Allow for this routine" }).click();
  await page
    .getByRole("dialog", { name: "Triage issues needs access" })
    .waitFor({ state: "detached" });

  // R9: Settings -> Routines.
  const settingsButton = page.getByRole("button", {
    name: "Settings",
    exact: true,
  });
  if (await settingsButton.isVisible().catch(() => false))
    await settingsButton.click();
  else
    // A narrow window hides the sidebar: ask the shell the way a dialog does.
    await page.evaluate(() =>
      window.dispatchEvent(
        new CustomEvent("sushiai:open-settings", { detail: "connections" }),
      ),
    );
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("tab", { name: "Routines", exact: true }).click();
  await page.locator(".rt-settings").waitFor();
  await capture(page, "R9", "settings", { selector: "[role=dialog]" });
  if (!tag) {
    // The three owner choices persist through the builder settings.
    await page.getByRole("checkbox", { name: "Connections" }).click();
    await page.getByRole("radio", { name: "Both" }).click();
    report.settingsAfter = await app.evaluate(() => globalThis.__rt.settings);
  }
  await page.keyboard.press("Escape");
  await page.waitForTimeout(250);
}

try {
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");
  const size = (w, h) =>
    app.evaluate(
      ({ BrowserWindow }, [width, height]) => {
        BrowserWindow.getAllWindows()[0].setContentSize(width, height);
      },
      [w, h],
    );
  await size(1380, 864);
  await page.waitForTimeout(300);

  // Extension starts off.
  report.rowBeforeEnable = await page
    .locator(".sidebar")
    .getByText("Routines", { exact: true })
    .count();
  // Routines is on from the start; enable it only when an older build left it off.
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  const enable = page.getByRole("button", { name: "Enable Routines" });
  if (await enable.count()) {
    await enable.click();
    await page.getByRole("button", { name: "Disable Routines" }).waitFor();
  }
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  await page.waitForTimeout(500);
  report.sidebarButtonsNamedRoutines = await page
    .locator(".sidebar")
    .getByRole("button", { name: "Routines", exact: true })
    .count();

  const open = async () => {
    // The sidebar button toggles the page: press it only while the page is
    // not on screen. A narrow window may hide the sidebar; then the page is
    // already the one shown.
    if (await page.locator(".rt-page").count()) return;
    const button = sidebarRoutines(page);
    if (await button.isVisible().catch(() => false)) await button.click();
    await page.waitForTimeout(250);
  };
  await open();
  await walk(page);

  // The Open session link: the run's session becomes a pane.
  await page.keyboard.press("Escape");
  await open();
  const sessionRun = run(11, "running", {
    startedAt: now - 90e3,
    endedAt: undefined,
    session: "sess-run",
  });
  const sessionState = listState({ intakeRuns: [sessionRun] });
  sessionState.entries = sessionState.entries.map((entry) =>
    entry === E.intake
      ? { ...E.intake, lastRun: sessionRun, running: true }
      : entry,
  );
  await put({
    ...sessionState,
    sessions: [{ id: "sess-run", asks: [] }],
    screens: { "sess-run": "[09:00:12] read 14 open issues of my-app" },
  });
  await pick(page, "Triage issues");
  await page.locator(".rt-runs button").first().click();
  const opener = page.getByRole("button", { name: "Open session" });
  await opener
    .first()
    .waitFor({ timeout: 5000 })
    .catch(() => {});
  report.openSessionButton = await opener.count();
  if (!report.openSessionButton)
    report.openSessionDom = await page
      .locator(".rt-detail")
      .first()
      .evaluate((el) => el.outerHTML.slice(0, 1500))
      .catch((e) => String(e));
  if (report.openSessionButton) {
    await opener.click();
    await page.waitForTimeout(600);
    report.openSessionResult = await page.evaluate(() => ({
      routinesPageStillShown: !!document.querySelector(".rt-page"),
      paneTitles: [...document.querySelectorAll(".panel-title, .panel-header")]
        .map((el) => el.textContent?.trim())
        .filter(Boolean)
        .slice(0, 6),
    }));
    await page.screenshot({ path: shot("open-session") });
  }

  // The width sweep.
  for (const width of WIDTHS) {
    await size(width, width === MIN_WIDTH ? Math.max(MIN_HEIGHT, 760) : 860);
    await page.waitForTimeout(400);
    tag = `w${width}-`;
    await open();
    await walk(page);
    await page.keyboard.press("Escape");
  }
  tag = "";

  report.window = await app.evaluate(({ BrowserWindow }) => {
    const win = BrowserWindow.getAllWindows()[0];
    const [width, height] = win.getContentSize();
    return {
      isVisible: win.isVisible(),
      isFocused: win.isFocused(),
      contentSize: { width, height },
    };
  });
  report.stubCalls = await app.evaluate(() => [
    ...new Set(globalThis.__rt.calls),
  ]);
  report.stubArgs = await app.evaluate(() =>
    globalThis.__rt.args
      .filter(([m]) =>
        ["grant", "approve", "open-login-terminal", "settings.set"].includes(m),
      )
      .slice(0, 8),
  );
} catch (error) {
  report.error = String(error?.stack ?? error);
  try {
    const page = await app.firstWindow();
    await page.screenshot({ path: shot("ERR-last-state") });
  } catch {}
} finally {
  await app.close();
  stopDaemon(`${profile}/sushiai`);
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
