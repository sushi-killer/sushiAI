// Typed wrapper over the generic `window.bridge.orchestrator(method, params)`
// passthrough - one place that knows the request/response shape of each
// method in the protocol table, so OrchestratorPanel/Settings never spell out
// a method string or cast a result themselves.
import type {
  ChatSessionList,
  ChatThread,
  FailureRow,
  Message,
  Proposal,
  Settings,
  Task,
  TimelineSegment,
} from "./types";

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
  settingsDefaults: () => call<Settings>("settings.defaults"),
  taskList: (repo?: string, includeArchived?: boolean) =>
    call<Task[]>("task.list", {
      ...(repo ? { repo } : {}),
      ...(includeArchived ? { includeArchived } : {}),
    }),
  taskGet: (id: string) => call<Task>("task.get", { id }),
  taskTimeline: (id: string) =>
    call<TimelineSegment[]>("task.timeline", { id }),
  failuresCatalogue: (repo?: string, sinceDays?: number) =>
    call<FailureRow[]>("failures.catalogue", {
      ...(repo ? { repo } : {}),
      ...(sinceDays ? { sinceDays } : {}),
    }),
  taskCreate: (
    repo: string,
    params: {
      request?: string;
      title?: string;
      goal?: string;
      criteria?: string[];
      verify?: string[];
      base?: string;
      start?: boolean;
    },
  ) => call<Task>("task.create", { repo, ...params }),
  taskStart: (id: string) => call<Task>("task.start", { id }),
  taskStop: (id: string) => call<Task>("task.stop", { id }),
  taskAnswer: (id: string, answer: string) =>
    call<Task>("task.answer", { id, answer }),
  taskOverturn: (id: string, index: number, answer: string) =>
    call<Task>("task.overturn", { id, index, answer }),
  taskDelete: (id: string) =>
    call<Record<string, never>>("task.delete", { id }),
  taskArchive: (id: string) => call<Task>("task.archive", { id }),
  taskUnarchive: (id: string) => call<Task>("task.unarchive", { id }),
  chatGet: (repo: string) => call<ChatThread>("chat.get", { repo }),
  chatSend: (repo: string, text: string) =>
    call<Record<string, never>>("chat.send", { repo, text }),
  chatCancel: (repo: string) =>
    call<Record<string, never>>("chat.cancel", { repo }),
  chatList: (repo: string) => call<ChatSessionList>("chat.list", { repo }),
  chatNew: (repo: string) => call<ChatThread>("chat.new", { repo }),
  chatSwitch: (repo: string, id: string) =>
    call<ChatThread>("chat.switch", { repo, id }),
  chatClear: (repo: string) => call<ChatThread>("chat.clear", { repo }),
  messageList: (repo: string) => call<Message[]>("message.list", { repo }),
  evolutionRun: () => call<unknown>("evolution.run"),
  evolutionList: (repo?: string) =>
    call<Proposal[]>("evolution.list", repo ? { repo } : {}),
  evolutionApprove: (id: string) => call<Proposal>("evolution.approve", { id }),
  evolutionReject: (id: string, reason?: string) =>
    call<Proposal>("evolution.reject", { id, ...(reason ? { reason } : {}) }),
  evolutionAdopt: (id: string) => call<Proposal>("evolution.adopt", { id }),
};
