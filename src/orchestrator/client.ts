// Typed wrapper over the generic `window.bridge.orchestrator(method, params)`
// passthrough - one place that knows the request/response shape of each
// method in the protocol table, so OrchestratorPanel/Settings never spell out
// a method string or cast a result themselves.
import type { ChatThread, Settings, Task } from "./types";

function call<T>(method: string, params?: Record<string, unknown>): Promise<T> {
  if (!window.bridge)
    return Promise.reject(new Error("Open the desktop app first."));
  return window.bridge.orchestrator(method, params) as Promise<T>;
}

export const orchestratorClient = {
  ping: () => call<{ version: string; pid: number; dataDir: string }>("ping"),
  settingsGet: () => call<Settings>("settings.get"),
  settingsSet: (settings: Settings) =>
    call<Settings>("settings.set", { settings }),
  taskList: (repo?: string) => call<Task[]>("task.list", repo ? { repo } : {}),
  taskGet: (id: string) => call<Task>("task.get", { id }),
  taskStart: (id: string) => call<Task>("task.start", { id }),
  taskStop: (id: string) => call<Task>("task.stop", { id }),
  taskAnswer: (id: string, answer: string) =>
    call<Task>("task.answer", { id, answer }),
  taskDelete: (id: string) =>
    call<Record<string, never>>("task.delete", { id }),
  chatGet: (repo: string) => call<ChatThread>("chat.get", { repo }),
  chatSend: (repo: string, text: string) =>
    call<Record<string, never>>("chat.send", { repo, text }),
  chatCancel: (repo: string) =>
    call<Record<string, never>>("chat.cancel", { repo }),
};
