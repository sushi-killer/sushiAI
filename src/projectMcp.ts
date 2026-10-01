export type ProjectMcpServer = {
  command?: string;
  args?: string[];
  url?: string;
  env?: Record<string, string>;
  headers?: Record<string, string>;
};

export function mcpVariableReferences(value: unknown): string[] {
  const references = new Set<string>();
  const visit = (item: unknown) => {
    if (typeof item === "string") {
      for (const match of item.matchAll(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g))
        references.add(match[1]);
    } else if (Array.isArray(item)) item.forEach(visit);
    else if (item && typeof item === "object")
      Object.values(item).forEach(visit);
  };
  visit(value);
  return [...references].sort((a, b) => a.localeCompare(b));
}

/** How many servers reference at least one secret variable (a server with two
 * secret headers counts once). */
export function serversUsingSecrets(
  servers: Record<string, unknown>,
  env: { name: string; secret?: boolean }[],
): number {
  const secrets = new Set(
    env.filter((entry) => entry.secret).map((entry) => entry.name),
  );
  return Object.values(servers).filter((server) =>
    mcpVariableReferences(server).some((name) => secrets.has(name)),
  ).length;
}

export function unknownMcpVariables(
  value: unknown,
  environment: { name: string }[],
): string[] {
  const names = new Set(environment.map((entry) => entry.name));
  return mcpVariableReferences(value).filter((name) => !names.has(name));
}

export function groupAlsoInProject<T extends { source: string }, P>(
  servers: T[],
  plugins: P[],
) {
  return {
    repo: servers.filter((server) => server.source === "project"),
    personal: servers.filter((server) => server.source !== "project"),
    plugins,
  };
}

/** "4 servers · 3 use secrets", with the right numbers and verbs. */
export function mcpCountLine(servers: number, secrets: number): string {
  const first = `${servers} ${servers === 1 ? "server" : "servers"}`;
  if (secrets === 0) return `${first} · no secrets`;
  return secrets === 1
    ? `${first} · 1 uses a secret`
    : `${first} · ${secrets} use secrets`;
}

/** "14 uses in 30 days", "1 use in 30 days", or nothing when there were none. */
export function usesText(count: number): string {
  return count > 0 ? `${count} ${count === 1 ? "use" : "uses"} in 30 days` : "";
}

/** The line under a plugin: what it ships (skills, servers) and how often it
 * was used. Only what is known is said. */
export function pluginLine(info: {
  skills: boolean;
  servers: number;
  uses: number;
}): string {
  const ships = [
    info.skills ? "skills" : "",
    info.servers > 0
      ? `${info.servers} ${info.servers === 1 ? "server" : "servers"}`
      : "",
  ]
    .filter(Boolean)
    .join(" and ");
  return ["Claude plugin", ships, usesText(info.uses)]
    .filter(Boolean)
    .join(" · ");
}

/** The name Claude Code gives a server inside a tool name: anything but
 * letters, digits, "_" and "-" becomes "_". */
export function claudeToolName(name: string): string {
  return name.replace(/[^a-zA-Z0-9_-]/g, "_");
}
