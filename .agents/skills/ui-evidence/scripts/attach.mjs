// Attach to the owner's running `npm run dev` window over CDP and look at it.
//
//   node .agents/skills/ui-evidence/scripts/attach.mjs shot <out.png> [--selector css] [--full]
//   node .agents/skills/ui-evidence/scripts/attach.mjs measure <css...>
//   node .agents/skills/ui-evidence/scripts/attach.mjs eval <expr>
//   node .agents/skills/ui-evidence/scripts/attach.mjs click <css>
//   node .agents/skills/ui-evidence/scripts/attach.mjs type <css> <text>
//   node .agents/skills/ui-evidence/scripts/attach.mjs key <combo>
//
// shot and measure only read. eval is for read-only expressions. click, type
// and key act on the owner's live data: ask first when they send, delete or
// archive. The window is never resized or moved unless --viewport WxH is
// given. The app has no route API, so there is no goto.
import { chromium } from "playwright";
import { fileURLToPath } from "node:url";
import {
  defaultHome,
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

/** The main app window: not DevTools, not the mascot. */
export function pickMainPage(pages) {
  const app = pages.filter((page) => {
    const url = page.url();
    return !url.startsWith("devtools:") && !/\/mascot\.html/.test(url);
  });
  return app[0] ?? null;
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
      await page.locator(rest[0]).first().fill(rest.slice(1).join(" "));
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
  const { wsEndpoint } = readDevtools(defaultHome());
  const browser = await chromium.connectOverCDP(wsEndpoint);
  try {
    const page = pickMainPage(browser.contexts().flatMap((c) => c.pages()));
    if (!page) throw new Error("no app window found");
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
