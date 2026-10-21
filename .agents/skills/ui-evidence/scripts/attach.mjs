// Attach to the owner's running `npm run dev` window over CDP and look at it.
// The endpoint is ~/.sushiai-dev/devtools.json; dev.mjs publishes it.
//
//   node .agents/skills/ui-evidence/scripts/attach.mjs shot <out.png> [--selector css] [--full]
//   node .agents/skills/ui-evidence/scripts/attach.mjs measure <css...>
//   node .agents/skills/ui-evidence/scripts/attach.mjs eval <expr>
//   node .agents/skills/ui-evidence/scripts/attach.mjs click <css>
//   node .agents/skills/ui-evidence/scripts/attach.mjs type <css> <text>   (appends at the end)
//   node .agents/skills/ui-evidence/scripts/attach.mjs key <combo>
//
// shot and measure only read. eval is for read-only expressions. click, type
// and key act on the owner's live data: ask first when they send, delete or
// archive. The window is never resized or moved unless --viewport WxH is
// given. The app has no route API, so there is no goto.
import { chromium } from "playwright";
import { fileURLToPath } from "node:url";
import {
  DEV_URL,
  readDevtools,
} from "../../../../scripts/lib/devtools-endpoint.mjs";

const STYLES = [
  "display",
  "position",
  "color",
  "backgroundColor",
  "fontFamily",
  "fontSize",
  "fontWeight",
  "lineHeight",
  "borderRadius",
  "padding",
  "margin",
  "gap",
  "overflow",
];

function parseArgs(argv) {
  const flags = {};
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--full") flags.full = true;
    else if (arg === "--selector") flags.selector = argv[++i];
    else if (arg === "--viewport") flags.viewport = argv[++i];
    else rest.push(arg);
  }
  return { flags, rest };
}

/** The main app window: the page served at the dev URL root. DevTools, the
 * mascot (a different path) and anything else never match. A visible window
 * wins; a hidden one (a test run) is the fallback. Entries are
 * `{url, visible, ...}`. */
export function pickMainPage(entries, devUrl = DEV_URL) {
  const root = new URL(devUrl);
  const main = entries.filter(({ url }) => {
    let page;
    try {
      page = new URL(url);
    } catch {
      return false;
    }
    return (
      page.origin === root.origin &&
      (page.pathname === "/" || page.pathname === "/index.html")
    );
  });
  return main.find((entry) => entry.visible) ?? main[0] ?? null;
}

/** The port in the file must really be the debugger of that app. */
async function confirmEndpoint({ port, wsEndpoint }) {
  const response = await fetch(`http://127.0.0.1:${port}/json/version`);
  const { webSocketDebuggerUrl } = await response.json();
  if (webSocketDebuggerUrl !== wsEndpoint)
    throw new Error("devtools.json does not match the running debugger");
}

function viewportOf(text) {
  const match = /^(\d+)x(\d+)$/.exec(text ?? "");
  if (!match) throw new Error("--viewport takes WIDTHxHEIGHT, e.g. 1440x900");
  return { width: Number(match[1]), height: Number(match[2]) };
}

async function measure(page, selectors) {
  return page.evaluate(
    ([list, styles]) =>
      list.map((selector) => ({
        selector,
        matches: [...document.querySelectorAll(selector)].map((el) => {
          const box = el.getBoundingClientRect();
          const css = getComputedStyle(el);
          return {
            box: {
              x: box.x,
              y: box.y,
              width: box.width,
              height: box.height,
            },
            scroll: {
              scrollWidth: el.scrollWidth,
              clientWidth: el.clientWidth,
              scrollHeight: el.scrollHeight,
              clientHeight: el.clientHeight,
            },
            styles: Object.fromEntries(styles.map((name) => [name, css[name]])),
          };
        }),
      })),
    [selectors, STYLES],
  );
}

async function run(command, rest, flags, page) {
  switch (command) {
    case "shot": {
      const [out] = rest;
      if (!out) throw new Error("shot needs an output path");
      if (flags.selector)
        await page.locator(flags.selector).first().screenshot({ path: out });
      else await page.screenshot({ path: out, fullPage: !!flags.full });
      return { path: out };
    }
    case "measure":
      if (!rest.length) throw new Error("measure needs at least one selector");
      return measure(page, rest);
    case "eval":
      if (!rest[0]) throw new Error("eval needs an expression");
      return page.evaluate(rest[0]);
    case "click":
      await page.locator(rest[0]).first().click();
      return { clicked: rest[0] };
    case "type":
      if (rest.length < 2) throw new Error("type needs a selector and text");
      // Appends: the caret goes to the end, then keys are typed like a person.
      await page
        .locator(rest[0])
        .first()
        .evaluate((el) => {
          el.focus();
          if (el.setSelectionRange) {
            const end = el.value.length;
            el.setSelectionRange(end, end);
          } else {
            const range = document.createRange();
            range.selectNodeContents(el);
            range.collapse(false);
            const selection = getSelection();
            selection.removeAllRanges();
            selection.addRange(range);
          }
        });
      await page.keyboard.type(rest.slice(1).join(" "));
      return { typed: rest[0] };
    case "key":
      await page.keyboard.press(rest[0]);
      return { key: rest[0] };
    default:
      throw new Error(`unknown command: ${command ?? "(none)"}`);
  }
}

async function main() {
  const { flags, rest } = parseArgs(process.argv.slice(2));
  const [command, ...args] = rest;
  const info = readDevtools();
  await confirmEndpoint(info);
  const browser = await chromium.connectOverCDP(info.wsEndpoint);
  try {
    const entries = [];
    for (const page of browser.contexts().flatMap((c) => c.pages()))
      entries.push({
        page,
        url: page.url(),
        visible: await page
          .evaluate(() => document.visibilityState === "visible")
          .catch(() => false),
      });
    const found = pickMainPage(entries, process.env.BRIDGE_DEV_URL || DEV_URL);
    if (!found) throw new Error("no app window found");
    const { page } = found;
    if (flags.viewport) await page.setViewportSize(viewportOf(flags.viewport));
    console.log(JSON.stringify(await run(command, args, flags, page), null, 2));
  } finally {
    // Disconnects only; the owner's app keeps running.
    await browser.close();
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url))
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
