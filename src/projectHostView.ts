import type { Project, ProjectHostReadiness } from "./types";

/** What a status dot says: fine, worth a look, or blocking. `null` is "not
 * applicable yet" and draws a dash. */
export type Dot = "ok" | "warning" | "danger" | null;

export type HostDots = {
  checkout: Dot;
  setup: Dot;
  clis: Dot;
  mcp: Dot;
  secrets: Dot;
  /** The columns that need attention, in column order (checkout excluded:
   * a missing checkout is what Prepare fixes). */
  problems: ("setup" | "clis" | "mcp" | "secrets")[];
};

/** The five readiness dots of one host row, from a readiness check. */
export function hostDots(
  matrix: ProjectHostReadiness,
  trusted: boolean,
): HostDots {
  const checkout: Dot = matrix.checkout.ok ? "ok" : "danger";
  const setup: Dot = !matrix.checkout.ok
    ? null
    : matrix.setup.configured && !matrix.setup.stale
      ? "ok"
      : "warning";
  const { claude, codex } = matrix.clis;
  const clis: Dot =
    claude.installed || codex.installed
      ? claude.loggedIn || codex.loggedIn
        ? "ok"
        : "warning"
      : "danger";
  const mcp: Dot =
    matrix.mcp.count === 0 ? null : matrix.mcp.ok ? "ok" : "warning";
  const secrets: Dot = !trusted
    ? "danger"
    : matrix.secrets.count > 0
      ? "ok"
      : "warning";
  const problems = (
    [
      ["setup", setup],
      ["clis", clis],
      ["mcp", mcp],
      ["secrets", secrets],
    ] as const
  )
    .filter(([, dot]) => dot === "warning" || dot === "danger")
    .map(([name]) => name);
  return { checkout, setup, clis, mcp, secrets, problems };
}

/** This Mac's readiness, from what the project itself says: the checkout is
 * open here, and the agents are the ones found on this machine. */
export function localReadiness(
  project: Project,
  agents: { name: string; path: string | null }[],
  path: string,
): ProjectHostReadiness {
  const has = (name: string) =>
    agents.some((agent) => agent.name.toLowerCase() === name && agent.path);
  const servers = Object.keys(
    (project.mcp as { mcpServers?: Record<string, unknown> }).mcpServers ?? {},
  ).length;
  const secrets = project.env.filter(
    (entry) => entry.secret && entry.hasValue,
  ).length;
  return {
    checkout: { ok: true, path, nonStandard: false },
    setup: {
      ok: !!(project.setup.install || project.setup.check),
      configured: !!(project.setup.install || project.setup.check),
    },
    clis: {
      git: true,
      claude: { installed: has("claude"), loggedIn: has("claude") },
      codex: { installed: has("codex"), loggedIn: has("codex") },
      checkedAt: 0,
    },
    mcp: { ok: servers > 0, count: servers },
    secrets: { ok: secrets > 0, count: secrets },
    trusted: true,
  };
}

/** The override editor's text: `KEY=value` lines. */
export function overridesToText(overrides: Record<string, unknown>): string {
  const lines = Object.entries(overrides).map(
    ([key, value]) =>
      `${key}=${typeof value === "string" ? value : JSON.stringify(value)}`,
  );
  return lines.join("\n");
}

/** The object behind the editor's text; comments and blank lines are skipped. */
export function textToOverrides(text: string): Record<string, string> {
  const overrides: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const at = line.indexOf("=");
    if (at <= 0) throw new Error(`Line "${line}" is not KEY=value.`);
    const key = line.slice(0, at).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key))
      throw new Error(`"${key}" is not a valid variable name.`);
    overrides[key] = line.slice(at + 1);
  }
  return overrides;
}
