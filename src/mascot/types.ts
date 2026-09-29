import type { TaskNotice } from "../orchestrator/notices";

/** One queued notice as electron/mascot.cjs publishes it. */
export type MascotNotice = TaskNotice & {
  id: string;
  /** Epoch ms the notice leaves at; null while it waits for the owner. */
  expiresAt: number | null;
  /** A quick answer was sent; the bubble shows a short confirmation. */
  answered?: boolean;
};

export type MascotBridge = {
  onNotices(callback: (notices: MascotNotice[]) => void): () => void;
  answer(taskId: string, text: string): Promise<string>;
  open(taskId: string, focus: TaskNotice["focus"]): Promise<void>;
  dismiss(id: string): Promise<void>;
};

declare global {
  interface Window {
    mascot?: MascotBridge;
  }
}
