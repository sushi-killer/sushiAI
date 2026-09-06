import { Resvg } from "@resvg/resvg-js";
import fs from "node:fs/promises";
import { execFileSync } from "node:child_process";
const source = await fs.readFile("public/sushi.svg", "utf8");
const content = source.slice(
  source.indexOf(">") + 1,
  source.lastIndexOf("</svg>"),
);
const icon = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024"><defs><linearGradient id="surface" x2="0" y2="1"><stop stop-color="#26362c"/><stop offset="1" stop-color="#111813"/></linearGradient></defs><rect x="64" y="64" width="896" height="896" rx="204" fill="url(#surface)"/><rect x="65" y="65" width="894" height="894" rx="203" fill="none" stroke="#53694e" stroke-opacity=".4" stroke-width="2"/><g transform="translate(224 215) scale(24)">${content}</g></svg>`;
await fs.mkdir("build/sushiAI.iconset", { recursive: true });
await fs.writeFile("build/sushi-app.svg", icon);
const render = (size) =>
  new Resvg(icon, { fitTo: { mode: "width", value: size } }).render().asPng();
for (const size of [16, 32, 128, 256, 512]) {
  await fs.writeFile(
    `build/sushiAI.iconset/icon_${size}x${size}.png`,
    render(size),
  );
  await fs.writeFile(
    `build/sushiAI.iconset/icon_${size}x${size}@2x.png`,
    render(size * 2),
  );
}
await fs.writeFile("public/sushi-dock.png", render(1024));
execFileSync("iconutil", [
  "-c",
  "icns",
  "build/sushiAI.iconset",
  "-o",
  "build/icon.icns",
]);
console.log("Created sushiAI app icon and macOS ICNS.");
