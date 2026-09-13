export const agentTitle = (name: string) =>
  ({
    claude: "Claude Code",
    codex: "Codex",
    gemini: "Gemini CLI",
    "cursor-agent": "Cursor Agent",
  })[name] || name;
