const { installHermesRecovery } = require("./hermes-recovery.cjs");
const {
  installConversationSettings,
} = require("./hermes-conversation-settings.cjs");
const path = require("node:path");
const { HermesScheduler } = require("./hermes-scheduler.cjs");
const { randomUUID } = require("node:crypto");
const { text, object } = require("./registry.cjs");
const { HermesTransport } = require("./hermes-transport.cjs");
const { normalizeHistory, reduceEvent } = require("./hermes-events.cjs");
const { installHermesMcp } = require("./hermes-mcp.cjs");
const { installHermesAddons } = require("./hermes-addons.cjs");
const { ActivityStore } = require("./activity-store.cjs");
const { activityFor } = require("./hermes-activity.cjs");
const { installHermesMedia } = require("./hermes-media.cjs");
const { installHermesFiles } = require("./hermes-files.cjs");
const { installHermesGit } = require("./hermes-git.cjs");
const { installHermesCommands } = require("./hermes-commands.cjs");
const {
  validateAttachments,
  stageAttachments,
} = require("./hermes-attachments.cjs");

const empty = () => ({
  items: [],
  status: "idle",
  info: {},
  usage: {},
  requests: [],
});
const key = (agent, conversation) => JSON.stringify([agent, conversation]);
const segment = (value) => encodeURIComponent(text(value, "resource ID", 500));
const boundedInt = (value, fallback, max) => {
  if (value === undefined) return fallback;
  if (!Number.isInteger(value) || value < 0 || value > max)
    throw new Error("Invalid page offset.");
  return value;
};

class HermesProvider {
  constructor({ transportFactory, transportOptions = {}, activityFile } = {}) {
    this.descriptor = {
      apiVersion: 1,
      id: "hermes",
      name: "Hermes",
      description: "Your local Hermes agents",
      capabilities: [
        "conversations",
        "streaming",
        "reasoning",
        "tools",
        "interactions",
        "attachments",
      ],
      addons: [],
    };
    this.factory =
      transportFactory ||
      ((options) => new HermesTransport({ ...transportOptions, ...options }));
    this.transports = new Map();
    this.connectionStates = new Map();
    this.recoveries = new Map();
    this.agents = new Map();
    this.sessions = new Map();
    this.bindings = new Map();
    this.pending = new Map();
    this.opening = new Map();
    this.dirty = new Set();
    this.timer = null;
    this.closed = false;
    this.activityStore = new ActivityStore(activityFile);
    this.activities = this.activityStore.entries;
    this.activityIds = new Set(this.activities.map((a) => a.id));
    this.publish = () => {};
    this.operations = new Map([
      ["agents.list", () => this.listAgents()],
      ["agents.create", (i) => this.createAgent(i)],
      ["connection.recover", (i) => this.recover(i)],
      ["activity.list", () => structuredClone(this.activities)],
      ["activity.status", () => ({ error: this.activityStore.error })],
      [
        "activity.read",
        () => {
          this.activities = this.activities.map((a) => ({ ...a, read: true }));
          this.activityStore.save(this.activities);
          return { ok: true };
        },
      ],
      ["conversations.list", (i) => this.listConversations(i)],
      ["conversations.open", (i) => this.open(i)],
      ["conversations.create", (i) => this.create(i)],
      ["conversations.canonical", (i) => this.canonical(i)],
      ["conversations.snapshot", (i) => this.snapshot(i)],
      ["conversations.send", (i) => this.send(i)],
      ["conversations.interrupt", (i) => this.interrupt(i)],
      ["conversations.update", (i) => this.update(i)],
      ["conversations.delete", (i) => this.remove(i)],
      ["conversations.history", (i) => this.history(i)],
      ["interactions.respond", (i) => this.respond(i)],
    ]);
    installHermesAddons(this);
    installHermesMcp(this);
    installHermesRecovery(this);
    installConversationSettings(this);
    installHermesMedia(this);
    installHermesFiles(this);
    installHermesGit(this);
    installHermesCommands(this);
    this.scheduler = new HermesScheduler({
      factory: this.factory,
      file: activityFile
        ? path.join(path.dirname(activityFile), "hermes-scheduler.json")
        : undefined,
      publish: (event) => this.publish(event),
    });
    this.operations.set("addons.schedules.schedulerStatus", async (input) => {
      const agentId = await this.agent(input);
      const status = await this.transport(agentId).request(
        "GET",
        "/api/status",
        { profile: agentId },
      );
      return {
        ...this.scheduler.snapshot(),
        gatewayRunning: status.gateway_running === true,
        gatewayState: status.gateway_state || "unknown",
      };
    });
    this.operations.set("addons.schedules.schedulerSet", async (input) => {
      await this.agent(input);
      return this.scheduler.set(input.enabled);
    });
  }
  transport(profile) {
    if (this.closed) throw new Error("Hermes provider is closed.");
    if (!this.transports.has(profile)) {
      this.transports.set(
        profile,
        this.factory({
          profile,
          onEvent: (event) => this.event(profile, event),
          onState: (state) => this.connectionState(profile, state),
        }),
      );
    }
    return this.transports.get(profile);
  }
  connectionState(agentId, state) {
    this.connectionStates.set(agentId, state);
    this.publish({ type: "connection", agentId, state });
    if (state.state !== "error" || this.closed) return;
    for (const s of this.sessions.values())
      if (s.agentId === agentId) {
        s.historyGap = true;
        s.submitting = false;
        s.state = { ...s.state, status: "error", requests: [] };
        this.changed(s);
      }
    if (
      ["CHILD_EXIT", "CHILD_ERROR"].includes(state.code) &&
      !this.recoveries.has(agentId)
    )
      queueMicrotask(() => {
        if (!this.closed) void this.recover({ agentId }).catch(() => {});
      });
  }
  async recover(input) {
    const agentId = text(input.agentId, "agent ID");
    if (!this.agents.has(agentId) && !this.transports.has(agentId))
      throw new Error("Unknown agent.");
    if (this.recoveries.has(agentId)) return this.recoveries.get(agentId);
    if (this.connectionStates.get(agentId)?.state !== "error")
      return { ok: true };
    const job = (async () => {
      await this.transports.get(agentId)?.close();
      if (this.closed) throw new Error("Agent provider is closed.");
      this.transports.delete(agentId);
      const sessions = [...this.sessions.values()].filter(
        (s) => s.agentId === agentId,
      );
      await this.transport(agentId).start();
      for (const s of sessions) {
        s.historyGap = true;
        s.submitting = false;
        s.state = {
          ...s.state,
          requests: [],
          info: { ...s.state.info, _transcript: undefined },
        };
        await this.reattach(s);
        s.state = {
          ...s.state,
          items: [
            ...s.state.items,
            {
              id: `recovered-${randomUUID()}`,
              kind: "notice",
              source: "sushiai",
              text: s.autoContinuing
                ? "Hermes restarted and is continuing the interrupted task according to its recovery settings."
                : "Hermes restarted. Saved history has been restored.",
            },
          ],
        };
        this.changed(s);
      }
      return { ok: true };
    })();
    this.recoveries.set(agentId, job);
    try {
      return await job;
    } catch (error) {
      this.connectionStates.set(agentId, {
        state: "error",
        code: "RECOVERY_FAILED",
      });
      this.publish({
        type: "connection",
        agentId,
        state: { state: "error", code: "RECOVERY_FAILED" },
      });
      for (const s of this.sessions.values())
        if (s.agentId === agentId) {
          s.state = {
            ...s.state,
            status: "error",
            items: [
              ...s.state.items,
              {
                id: `recover-error-${randomUUID()}`,
                kind: "notice",
                source: "sushiai",
                status: "error",
                text: "Could not restore Hermes. Use Reconnect to try again.",
              },
            ],
          };
          this.changed(s);
        }
      throw error;
    } finally {
      this.recoveries.delete(agentId);
    }
  }
  async agent(input) {
    const agentId = text(input.agentId, "agent ID");
    if (!this.agents.size) await this.listAgents();
    if (!this.agents.has(agentId))
      throw new Error("Agent no longer exists. Refresh the agent list.");
    return agentId;
  }
  async listAgents() {
    const result = await this.transport("default").request(
      "GET",
      "/api/profiles",
    );
    if (!Array.isArray(result.profiles))
      throw new Error("Hermes returned an unsupported profile list.");
    const agents = result.profiles.map((p) => ({
      id: text(p.name, "profile name"),
      providerId: "hermes",
      name: p.display_name || p.name,
      description: p.description || "",
      model: p.model || "",
      modelProvider: p.provider || "",
      skillCount: p.skill_count || 0,
    }));
    this.agents = new Map(agents.map((a) => [a.id, a]));
    return agents;
  }
  async createAgent(input) {
    const name = text(input.name, "agent name", 80);
    if (
      !/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(name) ||
      ["default", "current"].includes(name)
    )
      throw new Error(
        "Use a unique name with letters, numbers, hyphens or underscores.",
      );
    const description =
      typeof input.description === "string"
        ? input.description.slice(0, 4000)
        : "";
    const soul =
      typeof input.instructions === "string"
        ? input.instructions.slice(0, 200000)
        : "";
    const clone = input.cloneFrom
      ? await this.agent({ agentId: input.cloneFrom })
      : undefined;
    const result = await this.transport("default").rpc(
      "default",
      "profiles.create",
      {
        name,
        description,
        soul,
        ...(clone ? { clone_from: clone } : {}),
        clone_all: false,
        share_auth: true,
        mirror_credentials: true,
      },
    );
    if (result.ok === false || result.error)
      throw new Error("Hermes could not create this agent.");
    const agents = await this.listAgents();
    const created = agents.find((a) => a.id === name);
    if (!created)
      throw new Error(
        "Agent creation could not be confirmed. Refresh before trying again.",
      );
    return created;
  }
  async listConversations(input) {
    const agentId = await this.agent(input);
    const offset = boundedInt(input.offset, 0, 1000000);
    const query =
      input.search === undefined
        ? ""
        : text(input.search, "search", 500).trim();
    const result = await this.transport(agentId).request(
      "GET",
      query ? "/api/sessions/search" : "/api/sessions",
      {
        profile: agentId,
        query: query
          ? { q: query, limit: 100 }
          : {
              limit: 50,
              offset,
              order: "recent",
              archived: input.archived ? "only" : "exclude",
            },
      },
    );
    const rows = query ? result.results : result.sessions;
    if (!Array.isArray(rows))
      throw new Error("Hermes did not return a conversation list.");
    return {
      total: query ? rows.length : result.total,
      nextOffset:
        !query && offset + rows.length < result.total
          ? offset + rows.length
          : null,
      searchLimited: !!query && rows.length === 100,
      conversations: rows.map((s) => ({
        id: s.id || s.session_id,
        agentId,
        title: s.title || s.preview || "Untitled conversation",
        source: s.source,
        model: s.model,
        updatedAt: s.last_active || s.started_at,
        messageCount: s.message_count || 0,
        archived: !!s.archived,
        pinned: !!s.pinned,
        snippet: query ? s.snippet : undefined,
      })),
    };
  }
  get(input) {
    const id = key(
      text(input.agentId, "agent ID"),
      text(input.conversationId, "conversation ID", 500),
    );
    const session = this.sessions.get(id);
    if (!session) throw new Error("Open this conversation first.");
    return session;
  }
  snapshot(input) {
    const s = this.get(input);
    return structuredClone({
      agentId: s.agentId,
      conversationId: s.id,
      title: s.title,
      ...s.state,
      hasEarlier: s.hasEarlier,
      historyOffset: s.historyOffset,
    });
  }
  changed(session) {
    this.dirty.add(key(session.agentId, session.id));
    if (this.timer) return;
    // Keep token bursts out of React's render loop without delaying interaction cards.
    this.timer = setTimeout(() => {
      this.timer = null;
      for (const id of this.dirty) {
        const s = this.sessions.get(id);
        if (s)
          this.publish({
            type: "conversation",
            ...this.snapshot({ agentId: s.agentId, conversationId: s.id }),
          });
      }
      this.dirty.clear();
    }, 40);
    this.timer.unref?.();
  }
  event(agentId, event) {
    if (this.closed || !event || typeof event.type !== "string") return;
    const runtime = event.session_id;
    if (!runtime) {
      this.publish({ type: "provider-event", agentId, event });
      return;
    }
    const binding = key(agentId, runtime);
    const s = this.bindings.get(binding);
    if (!s) {
      // Events can precede the create/resume RPC result on the same socket.
      if (this.pending.size >= 50 && !this.pending.has(binding))
        this.pending.delete(this.pending.keys().next().value);
      const pending = this.pending.get(binding) || [];
      if (pending.length < 500) pending.push(event);
      this.pending.set(binding, pending);
      return;
    }
    if (event.type === "transport.reconnected") {
      void this.reattach(s).catch((error) => {
        s.state = {
          ...s.state,
          status: "error",
          items: [
            ...s.state.items,
            {
              id: `connection-${randomUUID()}`,
              kind: "notice",
              source: "sushiai",
              text: `Reconnect failed: ${error.message}`,
              status: "error",
            },
          ],
        };
        this.changed(s);
      });
      return;
    }
    if (event.type === "transport.resync") {
      s.historyGap = true;
      s.state = {
        ...s.state,
        items: [
          ...s.state.items,
          {
            id: `gap-${randomUUID()}`,
            kind: "notice",
            source: "sushiai",
            text: "Connection interrupted. Some live activity may be missing; saved history will be refreshed when the turn finishes.",
          },
        ],
      };
      this.changed(s);
      if (event.payload?.reason === "backend-restarted")
        void this.reattach(s).catch((error) => {
          s.state = { ...s.state, status: "error" };
          this.changed(s);
        });
      return;
    }
    s.state = reduceEvent(s.state, event);
    const toolId =
      event.payload?.tool_id ||
      event.payload?.tool_call_id ||
      event.payload?.call_id ||
      event.payload?.id;
    const tool = toolId
      ? s.state.items.find((i) => i.kind === "tool" && i.id === toolId)
      : s.state.items.at(-1);
    const activity = activityFor(event, tool);
    if (activity) {
      if (tool && event.type === "tool.complete")
        s.state = {
          ...s.state,
          items: s.state.items.map((i) =>
            i === tool ? { ...i, effect: activity.title } : i,
          ),
        };
      const activityId = key(
        agentId,
        `${s.runtime}:${s.id}:${event.type}:${event.seq || toolId || randomUUID()}`,
      );
      if (!this.activityIds.has(activityId)) {
        this.activityIds.add(activityId);
        const entry = {
          ...activity,
          id: activityId,
          agentId,
          conversationId: s.id,
          conversationTitle: s.title,
          agentName: this.agents.get(agentId)?.name || agentId,
          createdAt: Date.now(),
          read: false,
        };
        this.activities = [...this.activities, entry].slice(-500);
        this.activityStore.save(this.activities);
        if (this.activityIds.size > 1000)
          this.activityIds = new Set(this.activities.map((a) => a.id));
        this.publish({ type: "activity", activity: entry });
      }
    }
    if (event.type === "message.complete" || event.type === "error") {
      s.submitting = false;
      if (s.historyGap) void this.refreshHistory(s).catch(() => {});
    }
    this.changed(s);
  }
  async reattach(s) {
    if (s.reattaching) return s.reattaching;
    s.reattaching = (async () => {
      const r = await this.transport(s.agentId).rpc(
        s.agentId,
        "session.resume",
        {
          session_id: s.id,
          eager_build: true,
          omit_messages: true,
          close_on_disconnect: false,
          profile: s.agentId,
        },
      );
      if (typeof r.session_id !== "string" || !r.session_id)
        throw new Error("Hermes did not return a session binding.");
      if (r.session_id !== s.runtime) {
        this.bindings.delete(key(s.agentId, s.runtime));
        s.runtime = r.session_id;
        this.bindings.set(key(s.agentId, s.runtime), s);
      }
      s.autoContinuing = !!r.auto_continue;
      s.state = {
        ...s.state,
        info: { ...s.state.info, ...r.info },
        status: r.running || r.auto_continue ? "running" : "idle",
      };
      this.restoreInteractions(s, r);
      const binding = key(s.agentId, s.runtime);
      const pending = this.pending.get(binding) || [];
      this.pending.delete(binding);
      for (const event of pending) this.event(s.agentId, event);
      if (!r.running && !r.auto_continue && s.historyGap)
        await this.refreshHistory(s);
      this.changed(s);
    })();
    try {
      return await s.reattaching;
    } finally {
      s.reattaching = null;
    }
  }
  restoreInteractions(s, r) {
    for (const [field, type] of [
      ["pending_approval", "approval.request"],
      ["pending_clarify", "clarify.request"],
    ])
      if (r[field])
        s.state = reduceEvent(s.state, {
          type,
          session_id: s.runtime,
          payload:
            field === "pending_clarify" && Array.isArray(r[field].questions)
              ? {
                  ...r[field],
                  questions: r[field].questions.filter(
                    (q) =>
                      !Object.prototype.hasOwnProperty.call(
                        r[field].answers || {},
                        q.qid,
                      ),
                  ),
                }
              : r[field],
        });
  }
  async refreshHistory(s) {
    const r = await this.transport(s.agentId).request(
      "GET",
      `/api/sessions/${segment(s.id)}/messages`,
      { profile: s.agentId, query: { limit: 100, order: "latest" } },
    );
    s.state = {
      ...s.state,
      items: [
        ...normalizeHistory(r.messages),
        ...s.state.items.filter((item) => item.source === "sushiai"),
      ],
    };
    s.historyOffset = r.messages.length;
    s.hasEarlier = r.messages.length === 100;
    s.historyGap = false;
    this.changed(s);
  }
  bind(agentId, id, result, title, history) {
    if (typeof result.session_id !== "string")
      throw new Error("Hermes did not return a session binding.");
    const s = {
      agentId,
      id,
      runtime: result.session_id,
      autoContinuing: !!result.auto_continue,
      title: title || "New conversation",
      state: {
        ...empty(),
        items: normalizeHistory(history || result.messages || []),
        info: result.info || {},
        status: result.running || result.auto_continue ? "running" : "idle",
      },
      historyOffset: history?.length || 0,
      hasEarlier: history?.length === 100,
      submitting: false,
    };
    this.sessions.set(key(agentId, id), s);
    this.restoreInteractions(s, result);
    const binding = key(agentId, s.runtime);
    this.bindings.set(binding, s);
    const pending = this.pending.get(binding) || [];
    this.pending.delete(binding);
    for (const event of pending) this.event(agentId, event);
    this.changed(s);
    return this.snapshot({ agentId, conversationId: id });
  }
  async open(input) {
    const agentId = await this.agent(input);
    const id = text(input.conversationId, "conversation ID", 500);
    const identity = key(agentId, id);
    if (this.sessions.has(identity)) return this.snapshot(input);
    if (this.opening.has(identity)) return this.opening.get(identity);
    const job = (async () => {
      const t = this.transport(agentId);
      const history = await t.request(
        "GET",
        `/api/sessions/${segment(id)}/messages`,
        {
          profile: agentId,
          query: { limit: 100, order: "latest" },
        },
      );
      const result = await t.rpc(agentId, "session.resume", {
        session_id: history.session_id || id,
        profile: agentId,
        eager_build: true,
        omit_messages: true,
        close_on_disconnect: false,
      });
      return this.bind(
        agentId,
        id,
        result,
        input.title,
        history.messages || [],
      );
    })();
    this.opening.set(identity, job);
    try {
      return await job;
    } finally {
      this.opening.delete(identity);
    }
  }
  async create(input) {
    const agentId = await this.agent(input);
    // Hermes names an untitled conversation from its first message and keeps
    // improving it; a title stored here outranks that forever and also forces a
    // row for a draft the user may never type into. Only a title the user chose
    // is stored, and only that path needs the save check.
    const title =
      typeof input.title === "string"
        ? text(input.title, "conversation title", 200)
        : "";
    const result = await this.transport(agentId).rpc(
      agentId,
      "session.create",
      {
        profile: agentId,
        ...(title ? { title } : {}),
        close_on_disconnect: false,
      },
    );
    const id = text(result.stored_session_id, "stored conversation ID", 500);
    if (title)
      try {
        const saved = await this.transport(agentId).rpc(
          agentId,
          "session.title",
          { session_id: result.session_id, title },
        );
        if (saved.pending)
          throw new Error(
            "The conversation could not be saved. Refresh history before trying again.",
          );
      } catch (error) {
        await this.transport(agentId)
          .rpc(agentId, "session.close", { session_id: result.session_id })
          .catch(() => {});
        throw error;
      }
    return this.bind(agentId, id, result, title);
  }
  async canonical(input) {
    const agentId = await this.agent(input),
      identity = key(agentId, "canonical");
    if (this.opening.has(identity)) return this.opening.get(identity);
    const job = (async () => {
      const t = this.transport(agentId);
      const lookup = async () => {
        const result = await t.rpc(agentId, "session.list", {
          title: "Bot Chat",
          include_hidden: true,
          limit: 200,
        });
        if (!Array.isArray(result.sessions))
          throw new Error("Could not check the primary conversation.");
        return result.sessions.find(
          (s) => (s.root_title || s.title) === "Bot Chat",
        );
      };
      const existing = await lookup();
      if (existing)
        return this.open({
          agentId,
          conversationId: existing.resolved_id || existing.id,
          title: "Bot Chat",
        });
      const result = await t.rpc(agentId, "session.create", {
        profile: agentId,
        title: "Bot Chat",
        hidden: true,
        follow_profile_config: true,
        close_on_disconnect: false,
      });
      try {
        const title = await t.rpc(agentId, "session.title", {
          session_id: result.session_id,
          title: "Bot Chat",
        });
        if (title.pending)
          throw new Error("The primary conversation could not be saved yet.");
      } catch (error) {
        // The unique native title is the authority if another application won.
        const winner = await lookup();
        await t
          .rpc(agentId, "session.close", { session_id: result.session_id })
          .catch(() => {});
        if (winner)
          return this.open({
            agentId,
            conversationId: winner.resolved_id || winner.id,
            title: "Bot Chat",
          });
        throw error;
      }
      return this.bind(
        agentId,
        text(result.stored_session_id, "stored conversation ID", 500),
        result,
        "Bot Chat",
      );
    })();
    this.opening.set(identity, job);
    try {
      return await job;
    } finally {
      this.opening.delete(identity);
    }
  }
  async send(input, { displayText } = {}) {
    const s = this.get(input);
    if (s.attachmentStateUncertain)
      throw Error(
        "Restart sushiAI before sending again: the previous attachment upload could not be confirmed.",
      );
    if (s.settingsPending)
      throw Error("Wait for the conversation settings to finish saving.");
    const files = validateAttachments(input.attachments);
    let prompt = text(
      input.text || (files.length ? "Please review the attached files." : ""),
      "message",
      200000,
    );
    if (
      s.submitting ||
      ["running", "waiting", "working", "sending"].includes(s.state.status)
    )
      throw new Error(
        "Wait for this turn to finish or stop it before sending another message.",
      );
    s.submitting = true;
    const item = {
      id: `user-${randomUUID()}`,
      kind: "text",
      role: "user",
      text: displayText || prompt,
    };
    s.state = {
      ...s.state,
      status: "sending",
      items: [...s.state.items, item],
    };
    this.changed(s);
    let staged;
    try {
      staged = await stageAttachments(
        (method, params) =>
          this.transport(s.agentId).rpc(s.agentId, method, params),
        s.runtime,
        files,
      );
      if (staged.text) {
        prompt += `\n\n${staged.text}`;
        s.state = {
          ...s.state,
          items: s.state.items.map((row) =>
            row.id === item.id
              ? {
                  ...row,
                  text: `${item.text}\n\n${files.map((f) => `Attached: ${f.name}`).join("\n")}`,
                }
              : row,
          ),
        };
        s.state = {
          ...s.state,
          items: [
            ...s.state.items,
            ...staged.media.map((media, index) => ({
              id: `${item.id}-image-${index}`,
              kind: "image",
              role: "user",
              ...media,
            })),
          ],
        };
        this.changed(s);
      }
      await this.transport(s.agentId).rpc(s.agentId, "prompt.submit", {
        session_id: s.runtime,
        text: prompt,
      });
      if (s.state.status === "sending")
        s.state = { ...s.state, status: "running" };
    } catch (error) {
      if (files.length && !staged) s.attachmentStateUncertain = true;
      if (staged) {
        try {
          await staged.detach();
        } catch {
          s.attachmentStateUncertain = true;
        }
      }
      // A timeout may follow an accepted send. Keep it visible, never resend it.
      s.state = {
        ...s.state,
        status: "error",
        items: [
          ...s.state.items,
          {
            id: `error-${randomUUID()}`,
            kind: "notice",
            source: "sushiai",
            text: `Send could not be confirmed: ${error.message}. ${s.attachmentStateUncertain ? "Restart sushiAI before sending again to clear unconfirmed attachments." : "Check the conversation before retrying."}`,
            status: "error",
          },
        ],
      };
      s.submitting = false;
      throw error;
    } finally {
      this.changed(s);
    }
    return this.snapshot(input);
  }
  async interrupt(input) {
    const s = this.get(input);
    await this.transport(s.agentId).rpc(s.agentId, "session.interrupt", {
      session_id: s.runtime,
    });
    s.submitting = false;
    s.state = { ...s.state, status: "idle", requests: [] };
    this.changed(s);
    return this.snapshot(input);
  }
  async update(input) {
    const agentId = await this.agent(input),
      id = text(input.conversationId, "conversation ID", 500);
    const patch = object(input.patch),
      body = { profile: agentId };
    for (const field of ["archived", "pinned"]) {
      if (field in patch) {
        if (typeof patch[field] !== "boolean")
          throw new Error("Invalid conversation flag.");
        body[field] = patch[field];
      }
    }
    if ("title" in patch)
      body.title = text(patch.title, "conversation title", 200);
    const result = await this.transport(agentId).request(
      "PATCH",
      `/api/sessions/${segment(id)}`,
      { body },
    );
    const s = this.sessions.get(key(agentId, id));
    if (s && body.title) {
      s.title = body.title;
      this.changed(s);
    }
    return result;
  }
  async remove(input) {
    const agentId = await this.agent(input),
      id = text(input.conversationId, "conversation ID", 500);
    const identity = key(agentId, id),
      s = this.sessions.get(identity);
    if (
      s &&
      (s.settingsPending ||
        s.submitting ||
        s.state.status === "running" ||
        s.state.requests.length)
    )
      throw new Error("Stop this conversation before deleting it.");
    const t = this.transport(agentId);
    if (s) await t.rpc(agentId, "session.close", { session_id: s.runtime });
    await t.request("DELETE", `/api/sessions/${segment(id)}`, {
      profile: agentId,
    });
    this.sessions.delete(identity);
    if (s) this.bindings.delete(key(agentId, s.runtime));
    return { deleted: true };
  }
  async history(input) {
    const s = this.get(input);
    if (s.loadingHistory) return s.loadingHistory;
    if (!s.hasEarlier) return this.snapshot(input);
    s.loadingHistory = (async () => {
      const result = await this.transport(s.agentId).request(
        "GET",
        `/api/sessions/${segment(s.id)}/messages`,
        {
          profile: s.agentId,
          query: { limit: 100, offset: s.historyOffset, order: "latest" },
        },
      );
      if (!Array.isArray(result.messages))
        throw new Error("Hermes did not return message history.");
      s.historyOffset += result.messages.length;
      s.hasEarlier = result.messages.length === 100;
      const merged = new Map(
        normalizeHistory(result.messages).map((item) => [item.id, item]),
      );
      for (const item of s.state.items)
        merged.set(item.id, { ...merged.get(item.id), ...item });
      s.state = { ...s.state, items: [...merged.values()] };
      this.changed(s);
      return this.snapshot(input);
    })();
    try {
      return await s.loadingHistory;
    } finally {
      s.loadingHistory = null;
    }
  }
  async respond(input) {
    const s = this.get(input),
      requestId = text(input.requestId, "interaction ID");
    const request = s.state.requests.find(
      (r) => (r.id || r.request_id) === requestId,
    );
    if (!request) throw new Error("This interaction is no longer pending.");
    const kind = request.kind || request.type;
    const response = object(input.response);
    const contracts = {
      approval: ["approval.respond", "choice"],
      clarify: ["clarify.respond", "answer"],
      sudo: ["sudo.respond", "password"],
      secret: ["secret.respond", "value"],
    };
    const contract = contracts[String(kind).replace(/\.request$/, "")];
    if (!contract)
      throw new Error("This interaction needs its dedicated add-on.");
    const value = response[contract[1]];
    if (typeof value !== "string" || value.length > 200000)
      throw new Error("Invalid interaction response.");
    const payload = request.input || request;
    if (contract[0] === "approval.respond") {
      const choices = Array.isArray(payload.choices)
        ? payload.choices
        : ["once", "session", "deny"];
      if (
        !["once", "session", "always", "deny"].includes(value) ||
        !choices.includes(value)
      )
        throw new Error("Invalid approval choice.");
    }
    const questions =
      contract[0] === "clarify.respond" && Array.isArray(payload.questions)
        ? payload.questions
        : [];
    const questionId =
      response.questionId == null
        ? null
        : text(response.questionId, "question ID");
    if (
      questions.length
        ? !questions.some((q) => q.qid === questionId)
        : questionId !== null
    )
      throw new Error("Invalid clarification question ID.");
    const result = await this.transport(s.agentId).rpc(s.agentId, contract[0], {
      session_id: s.runtime,
      request_id: requestId,
      [contract[1]]: value,
      ...(questionId ? { question_id: questionId } : {}),
    });
    const expired = result.status === "expired" || result.resolved === false;
    const remaining = Array.isArray(result.remaining) ? result.remaining : [];
    s.state = {
      ...s.state,
      requests: s.state.requests.flatMap((r) => {
        if (r !== request) return [r];
        if (expired || !remaining.length) return [];
        return [
          {
            ...r,
            input: {
              ...payload,
              questions: questions.filter((q) => remaining.includes(q.qid)),
            },
          },
        ];
      }),
    };
    if (!s.state.requests.length && s.state.status === "waiting")
      s.state = { ...s.state, status: "running" };
    this.changed(s);
    if (expired)
      throw new Error(
        "This request expired or was already answered. Your response was not applied.",
      );
    return result;
  }
  async close() {
    this.closed = true;
    clearTimeout(this.timer);
    await Promise.allSettled(
      [...this.transports.values()].map((t) => t.close()),
    );
    await this.scheduler.close();
    await this.activityStore.close();
    this.sessions.clear();
    this.bindings.clear();
    this.pending.clear();
    this.transports.clear();
  }
}

module.exports = { HermesProvider };
