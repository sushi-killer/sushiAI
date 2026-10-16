// Measure the live page for Figma parity. Used by drivers, never run alone.
//
//   const { writeMeasure } = await import(`${root}/.agents/skills/ui-evidence/scripts/measure.mjs`);
//   await writeMeasure(page, "routines-start", ".workspace-canvas");   // 3 lines in a driver
//
// Writes artifacts/<name>.measure.json (text nodes and filled or bordered
// boxes, relative to the element matched by the selector) and returns the
// counts. Compare it with `node scripts/figma-parity.mjs <snapshot> <file>`.
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export async function writeMeasure(
  page,
  name,
  selector = "body",
  root = process.cwd(),
) {
  const { measureExpression } = await import(
    pathToFileURL(path.join(root, "scripts/figma-parity.mjs")).href
  );
  const result = await page.evaluate(measureExpression(selector));
  const file = path.join(root, "artifacts", `${name}.measure.json`);
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, `${JSON.stringify(result, null, 2)}\n`);
  return { file, texts: result.texts.length, boxes: result.boxes.length };
}
