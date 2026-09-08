import { useSyncExternalStore } from "react";
import { readAgentAttachments, type AgentAttachment } from "./AgentAttachments";

type DraftState = {
  files: Record<string, AgentAttachment[]>;
  loading: Record<string, boolean>;
  errors: Record<string, string>;
};
// Shared only within this renderer's lifetime. Files never enter localStorage.
// Upload completion can outlive the Agent view without losing its target draft.
let state: DraftState = { files: {}, loading: {}, errors: {} };
const listeners = new Set<() => void>();
const update = (next: DraftState) => { state = next; listeners.forEach((listener) => listener()); };
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const useAttachmentDrafts = () => useSyncExternalStore(subscribe, () => state);
export const attachmentIsLoading = (id: string) => Boolean(state.loading[id]);
export function setAttachmentFiles(id: string, files: AgentAttachment[]) {
  update({ ...state, files: { ...state.files, [id]: files } });
}
export function setAttachmentError(id: string, error: string) {
  update({ ...state, errors: { ...state.errors, [id]: error } });
}
export async function addAttachmentFiles(id: string, files: File[]) {
  if (!files.length) return;
  if (state.loading[id]) { setAttachmentError(id, "Wait for the current file read to finish."); return; }
  if ((state.files[id]?.length || 0) + files.length > 8) {
    setAttachmentError(id, "Attach up to 8 files per message."); return;
  }
  update({ ...state, loading: { ...state.loading, [id]: true }, errors: { ...state.errors, [id]: "" } });
  try {
    const loaded = await readAgentAttachments(files);
    setAttachmentFiles(id, [...(state.files[id] || []), ...loaded]);
  } catch (error) { setAttachmentError(id, error instanceof Error ? error.message : String(error)); }
  finally { update({ ...state, loading: { ...state.loading, [id]: false } }); }
}
