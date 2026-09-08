import type { AgentTranscriptItem } from "./types";

// Empty strings, empty objects and objects holding only those carry nothing to
// read: an agent that reports {"text": ""} should leave no bubble behind.
export const blank = (value: unknown): boolean =>
  value === undefined ||
  value === null ||
  (typeof value === "string" && !value.trim()) ||
  (typeof value === "object" &&
    Object.values(value as Record<string, unknown>).every(blank));

// A tool row is worth showing for its name alone, because its name and outcome
// are the information. Everything else needs words inside it, so a stream event
// such as an empty thinking.delta leaves no row, during the answer or after it.
export const visible = (item: AgentTranscriptItem) => {
  if (item.kind === "image") return true;
  if (item.kind === "tool")
    return ![item.effect, item.name, item.text, item.input, item.output].every(
      blank,
    );
  return !blank(item.text) || !blank(item.output);
};

// The agent names its tools and events for machines. These are the names people
// recognize; anything unlisted is unpunctuated and capitalized rather than
// dressed up, so a new tool still reads as words.
const NAMES: Record<string, string> = {
  memory: "Memory",
  skills_list: "Skills",
  skill_view: "Skill",
  skill_manage: "Skills",
  terminal: "Terminal",
  execute_code: "Code",
  process: "Process",
  read_file: "Read file",
  write_file: "Write file",
  search_files: "Search files",
  patch: "Edit file",
  web_search: "Web search",
  web_extract: "Web page",
  todo: "To-dos",
  todo_list: "To-dos",
  todos: "To-dos",
  delegate_task: "Subagent",
  subagent: "Subagent",
  files: "Files",
  skills: "Skills",
  web: "Web",
  error: "Error",
};
export const label = (raw: unknown) => {
  const value = typeof raw === "string" ? raw.trim() : "";
  if (!value) return "";
  return (
    NAMES[value] ||
    value.replace(/[._-]+/g, " ").replace(/^./, (c) => c.toUpperCase())
  );
};

// Consecutive non-text items (tools, reasoning, activity) collapse into one
// block so a long tool run does not spam the transcript; text stays separate.
export function groupItems(items: AgentTranscriptItem[]) {
  const out: (AgentTranscriptItem | AgentTranscriptItem[])[] = [];
  for (const item of items) {
    const last = out[out.length - 1];
    if (item.kind === "text" || item.kind === "image") out.push(item);
    else if (Array.isArray(last)) last.push(item);
    else out.push([item]);
  }
  return out.map((entry) =>
    Array.isArray(entry) && entry.length === 1 ? entry[0] : entry,
  );
}
