import { ListTodo } from "lucide-react";

/** How a task shows up wherever the owner reads it, shared by the Inbox rows
 * and the orchestrator pane: the mark names the source, so a leading "orch:"
 * tag on the title is dropped. */
export const TaskMark = ListTodo;
export const cleanTitle = (title: string) => title.replace(/^orch:\s*/i, "");
