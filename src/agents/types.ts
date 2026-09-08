export interface AgentAddon {
  id: string;
  name: string;
  description?: string;
  [key: string]: unknown;
}
export interface AgentProvider {
  apiVersion: number;
  id: string;
  name: string;
  description: string;
  capabilities: string[];
  addons: AgentAddon[];
}
export interface AgentIdentity {
  id: string;
  providerId: string;
  name: string;
  description: string;
  model: string;
  modelProvider: string;
  skillCount: number;
}
export interface AgentActivity {
  id: string;
  agentId: string;
  agentName: string;
  conversationId: string;
  conversationTitle: string;
  title: string;
  summary: string;
  kind: string;
  createdAt: number;
  read: boolean;
}
export interface AgentConversation {
  id: string;
  agentId: string;
  title: string;
  source?: string;
  model?: string;
  updatedAt?: number;
  messageCount: number;
  archived?: boolean;
  pinned?: boolean;
}
export interface AgentTranscriptItem {
  id: string;
  kind: "text" | "reasoning" | "tool" | "activity" | "notice" | "todo" | "image";
  role?: string;
  text?: string;
  name?: string;
  input?: unknown;
  output?: unknown;
  status?: string;
  category?: string;
  [key: string]: unknown;
}
export interface AgentInteraction {
  id?: string;
  request_id?: string;
  kind?: string;
  type?: string;
  [key: string]: unknown;
}
export interface AgentSnapshot {
  agentId: string;
  conversationId: string;
  title: string;
  items: AgentTranscriptItem[];
  status: string;
  info: Record<string, unknown>;
  usage: Record<string, unknown>;
  requests: AgentInteraction[];
  hasEarlier?: boolean;
}
export type AgentEvent = {
  providerId: string;
  type: string;
  agentId?: string;
  [key: string]: unknown;
};
export type AgentCall = <T = unknown>(
  provider: string,
  operation: string,
  input?: Record<string, unknown>,
) => Promise<T>;
