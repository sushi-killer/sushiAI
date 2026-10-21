import { execFileSync } from "node:child_process";

// The desktop smoke drives a real Electron window, which only the macOS CI
// job and a Mac run. Elsewhere (a Linux agent host) it could never pass, so
// every task there failed a check no change of its own could fix.
if (process.platform !== "darwin") {
  console.log(
    "test:desktop: skipped on " +
      process.platform +
      "; the desktop smoke runs in CI on macOS.",
  );
  process.exit(0);
}
for (const script of [
  "build-once.mjs",
  "smoke.mjs",
  "smoke-companion.mjs",
  "attach-smoke.mjs",
]) {
  execFileSync(process.execPath, [`scripts/${script}`], { stdio: "inherit" });
}
