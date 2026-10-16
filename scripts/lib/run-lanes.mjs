// Runs lanes side by side. Commands inside a lane run in order and stop at the first
// failure. Output is buffered per lane and printed grouped once the lane ends.
import { spawn } from "node:child_process";

function runCommand(command) {
  return new Promise((resolve) => {
    const child = spawn(command, { shell: true });
    let output = "";
    child.stdout.on("data", (chunk) => (output += chunk));
    child.stderr.on("data", (chunk) => (output += chunk));
    child.on("error", (error) => resolve({ code: 1, output: `${error}\n` }));
    child.on("close", (code) => resolve({ code: code ?? 1, output }));
  });
}

async function runLane(lane) {
  const started = Date.now();
  let output = "";
  let code = 0;
  for (const command of lane.commands) {
    output += `$ ${command}\n`;
    const result = await runCommand(command);
    output += result.output;
    if (result.code !== 0) {
      code = result.code;
      break;
    }
  }
  return {
    name: lane.name,
    code,
    output,
    seconds: (Date.now() - started) / 1000,
  };
}

export async function runLanes(lanes, { quiet = false } = {}) {
  const log = quiet ? () => {} : (text) => process.stdout.write(text);
  const results = await Promise.all(
    lanes.map(async (lane) => {
      const result = await runLane(lane);
      const status = result.code === 0 ? "ok" : `FAILED (${result.code})`;
      log(
        `\n=== ${result.name}: ${status} in ${result.seconds.toFixed(1)}s ===\n${result.output}`,
      );
      return result;
    }),
  );
  return results.some((result) => result.code !== 0) ? 1 : 0;
}
