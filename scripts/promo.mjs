import { chromium } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import http from "node:http";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { once } from "node:events";
const require = createRequire(import.meta.url);
const mock = require("../promo/mock.cjs");
const root = process.cwd();
await fs.mkdir("promo/assets", { recursive: true });
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".png": "image/png",
};
const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    const requested = decodeURIComponent(url.pathname);
    const file = path.resolve(
      root,
      "." + (requested === "/" ? "/dist/index.html" : requested),
    );
    if (!file.startsWith(root + path.sep)) throw Error();
    res.setHeader(
      "Content-Type",
      mime[path.extname(file)] || "application/octet-stream",
    );
    res.end(await fs.readFile(file));
  } catch {
    res.writeHead(404);
    res.end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ channel: "chrome", headless: true });
try {
  if (!process.argv.includes("--render-only")) {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      deviceScaleFactor: 2,
    });
    await context.addInitScript(mock);
    const page = await context.newPage();
    await page.goto(base + "/dist/index.html");
    await page.waitForSelector(".xterm-screen");
    await page.waitForTimeout(1200);
    await page.addStyleTag({
      content: ".titlebar-left{padding-left:24px}.titlebar{padding-left:16px}",
    });
    await page.screenshot({ path: "promo/assets/workspace.png" });
    await page.getByTitle("Files and Git", { exact: true }).click();
    await page
      .getByRole("button", { name: "Maximize Files & Git", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Git changes", exact: true })
      .click();
    await page.getByTitle("src/checkout/Checkout.tsx", { exact: true }).click();
    await page.waitForSelector(".diff-add");
    await page.screenshot({ path: "promo/assets/git.png" });
    await page.keyboard.press("Escape");
    await page
      .getByRole("button", { name: "Close Files & Git", exact: true })
      .click();
    await page.getByTitle("Switch to tabs", { exact: true }).click();
    await page.getByRole("tab").filter({ hasText: "Claude Code" }).click();
    await page.setViewportSize({ width: 740, height: 860 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: "promo/assets/tabs.png" });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.waitForTimeout(300);
    await page.screenshot({ path: "promo/assets/remote.png" });
    await context.close();
    console.log("Synthetic app screenshots captured.");
  }
  if (process.argv.includes("--capture-only")) process.exitCode = 0;
  else {
    const page = await browser.newPage({
      viewport: { width: 1920, height: 1080 },
      deviceScaleFactor: 1,
    });
    await page.goto(base + "/promo/index.html");
    await page.waitForFunction(() => window.renderFrame);
    await page.evaluate(() =>
      Promise.all(Array.from(document.images).map((i) => i.decode())),
    );
    const duration = 36,
      fps = 30;
    for (const t of [1.5, 5, 11.5, 18, 24, 30, 34]) {
      await page.evaluate((t) => window.renderFrame(t), t);
      await page.screenshot({ path: `promo/assets/frame-${t}.png` });
    }
    if (!process.argv.includes("--stills-only")) {
      const encoder = spawn(
        "ffmpeg",
        [
          "-y",
          "-loglevel",
          "error",
          "-f",
          "image2pipe",
          "-framerate",
          String(fps),
          "-vcodec",
          "png",
          "-i",
          "pipe:0",
          "-i",
          "promo/soundtrack.wav",
          "-c:v",
          "libx264",
          "-preset",
          "fast",
          "-crf",
          "18",
          "-pix_fmt",
          "yuv420p",
          "-c:a",
          "aac",
          "-b:a",
          "256k",
          "-af",
          "loudnorm=I=-15:TP=-1.5:LRA=8",
          "-ar",
          "48000",
          "-movflags",
          "+faststart",
          "-t",
          String(duration),
          "promo/sushiAI-launch.mp4",
        ],
        { stdio: ["pipe", "inherit", "inherit"] },
      );
      const done = once(encoder, "close");
      for (let frame = 0; frame < duration * fps; frame++) {
        await page.evaluate((t) => window.renderFrame(t), frame / fps);
        const png = await page.screenshot({ type: "png" });
        if (!encoder.stdin.write(png)) await once(encoder.stdin, "drain");
        if (frame % (fps * 6) === 0)
          console.log(`Rendered ${frame / fps}/${duration}s`);
      }
      encoder.stdin.end();
      const [code] = await done;
      if (code) throw Error("ffmpeg failed: " + code);
      console.log("Created promo/sushiAI-launch.mp4 (36s, 1080p, 30fps).");
    }
  }
} finally {
  await browser.close();
  server.closeAllConnections();
  await new Promise((r) => server.close(r));
}
