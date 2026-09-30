export type ProjectMcpServer = {
  command?: string;
  args?: string[];
  url?: string;
  env?: Record<string, string>;
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

export function unknownMcpVariables(
  value: unknown,
  environment: { name: string }[],
): string[] {
  const names = new Set(environment.map((entry) => entry.name));
  return mcpVariableReferences(value).filter((name) => !names.has(name));
}

export function importedMcpServers(value: unknown) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("The .mcp.json file must contain a JSON object.");
  const servers = (value as { mcpServers?: unknown }).mcpServers;
  if (!servers || typeof servers !== "object" || Array.isArray(servers))
    throw new Error("The .mcp.json file must contain an mcpServers object.");
  return servers as Record<string, ProjectMcpServer>;
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
