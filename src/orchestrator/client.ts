// Typed wrapper over the generic `window.bridge.orchestrator(method, params)`
// passthrough - one place that knows the request/response shape of each
// method in the protocol table, so OrchestratorPanel/Settings never spell out
// a method string or cast a result themselves.
import type {
  BacklogBucket,
  ChatKind,
  ChatSessionList,
  ChatThread,
  FailureRow,
  Message,
  Note,
  Proposal,
  Settings,
  SpendGroup,
  SpendSummary,
  Task,
  TimelineSegment,
} from "./types";

export const LOCAL = "local";

/** The typed client for one host's daemon: "local" or an "ssh:<id>" profile. */
export function orchestratorClientFor(host: string = LOCAL) {
  const call = <T>(
    method: string,
    params?: Record<string, unknown>,
  ): Promise<T> => {
    if (!window.bridge)
      return Promise.reject(new Error("Open the desktop app first."));
    return window.bridge.orchestrator(method, params, host) as Promise<T>;
  };
  return {
    ping: () => call<{ version: string; pid: number; dataDir: string }>("ping"),
    /** Liveness only: pings the connection in use, never starts, restarts or
     * provisions the daemon (the panel's interval check). */
    probe: (): Promise<{ pid: number }> =>
      window.bridge
        ? window.bridge.orchestratorProbe(host)
        : Promise.reject(new Error("Open the desktop app first.")),
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
    /** `sinceDays`, or an inclusive UTC `from`/`to` (YYYY-MM-DD) range -
     * never both. */
    costsSummary: (params: {
      repo?: string;
      taskId?: string;
      sinceDays?: number;
      from?: string;
      to?: string;
      groupBy: SpendGroup[];
    }) => call<SpendSummary>("costs.summary", params),
    taskEvidence: (id: string, path: string) =>
      call<{ dataUrl: string }>("task.evidence", { id, path }),
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
        source?: string;
        dependsOn?: string[];
        /** Parks the task in the planning backlog instead of starting it. */
        backlog?: { bucket: BacklogBucket; order?: number };
      },
    ) => call<Task>("task.create", { repo, ...params }),
    taskStart: (id: string) => call<Task>("task.start", { id }),
    /** Moves a task within or between backlog buckets; `null` takes it out.
     * An omitted order appends to the end of the bucket. */
    taskBacklog: (id: string, bucket: BacklogBucket | null, order?: number) =>
      call<Task>("task.backlog", {
        id,
        bucket,
        ...(order === undefined ? {} : { order }),
      }),
    taskLand: (id: string) => call<Task>("task.land", { id }),
    taskStop: (id: string) => call<Task>("task.stop", { id }),
    taskAnswer: (id: string, answer: string) =>
      call<Task>("task.answer", { id, answer }),
    taskOverturn: (id: string, index: number, answer: string) =>
      call<Task>("task.overturn", { id, index, answer }),
    taskReport: (id: string) =>
      call<{ id: string; report: string }>("task.report", { id }),
    /** `touched` omitted clears the mark. */
    taskLeadTouch: (id: string, touched?: boolean, note?: string) =>
      call<Task>("task.leadTouch", {
        id,
        ...(touched === undefined ? {} : { touched }),
        ...(note ? { note } : {}),
      }),
    taskDelete: (id: string) =>
      call<Record<string, never>>("task.delete", { id }),
    taskArchive: (id: string) => call<Task>("task.archive", { id }),
    taskUnarchive: (id: string) => call<Task>("task.unarchive", { id }),
    // Every chat method acts on one kind's sessions; `chat` when omitted.
    chatGet: (repo: string, kind: ChatKind = "chat") =>
      call<ChatThread>("chat.get", { repo, kind }),
    chatSend: (repo: string, text: string, kind: ChatKind = "chat") =>
      call<Record<string, never>>("chat.send", { repo, text, kind }),
    chatCancel: (repo: string, kind: ChatKind = "chat") =>
      call<Record<string, never>>("chat.cancel", { repo, kind }),
    chatList: (repo: string, kind: ChatKind = "chat") =>
      call<ChatSessionList>("chat.list", { repo, kind }),
    chatNew: (repo: string, kind: ChatKind = "chat") =>
      call<ChatThread>("chat.new", { repo, kind }),
    chatSwitch: (repo: string, id: string, kind: ChatKind = "chat") =>
      call<ChatThread>("chat.switch", { repo, id, kind }),
    chatClear: (repo: string, kind: ChatKind = "chat") =>
      call<ChatThread>("chat.clear", { repo, kind }),
    /** Drops a session's draft and keeps its messages: session `id`, or the
     * kind's current one. Returns the kind's current session. */
    chatClearDraft: (repo: string, kind: ChatKind = "chat", id?: string) =>
      call<ChatThread>("chat.clearDraft", {
        repo,
        kind,
        ...(id ? { id } : {}),
      }),
    messageList: (repo: string) => call<Message[]>("message.list", { repo }),
    messageSend: (params: {
      from: string;
      to: string;
      replyTo: string;
      text: string;
    }) => call<Message>("message.send", params),
    evolutionRun: () => call<unknown>("evolution.run"),
    evolutionList: (repo?: string) =>
      call<Proposal[]>("evolution.list", repo ? { repo } : {}),
    evolutionApprove: (id: string) =>
      call<Proposal>("evolution.approve", { id }),
    evolutionReject: (id: string, reason?: string) =>
      call<Proposal>("evolution.reject", { id, ...(reason ? { reason } : {}) }),
    evolutionAdopt: (id: string) => call<Proposal>("evolution.adopt", { id }),
    repoNotesList: (repo: string) =>
      call<{ repo: string; notes: Note[] }>("repo.notes.list", { repo }).then(
        (result) => result.notes,
      ),
    repoNotesAdd: (repo: string, text: string) =>
      call<Note>("repo.notes.add", { repo, text }),
    repoNotesRemove: (repo: string, id: string) =>
      call<{ removed: string }>("repo.notes.remove", { repo, id }).then(
        () => undefined,
      ),
  };
}

export type OrchestratorClient = ReturnType<typeof orchestratorClientFor>;

/** The local daemon's client. */
export const orchestratorClient = orchestratorClientFor(LOCAL);
