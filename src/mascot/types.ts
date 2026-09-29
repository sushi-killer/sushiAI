import type { TaskNotice } from "../orchestrator/notices";

type Queued = {
  id: string;
  /** Epoch ms the notice leaves at; null while it waits for the owner. */
  expiresAt: number | null;
};

export type TaskMascotNotice = TaskNotice &
  Queued & {
    /** A quick answer was sent; the bubble shows a short confirmation. */
    answered?: boolean;
  };

/** Dev only: main's electron/** changed; Restart relaunches Electron. */
export type CoreUpdateNotice = Queued & {
  kind: "core-update";
  title: string;
  body: string;
};

/** One queued notice as electron/mascot.cjs publishes it. */
export type MascotNotice = TaskMascotNotice | CoreUpdateNotice;

export type MascotBridge = {
  onNotices(callback: (notices: MascotNotice[]) => void): () => void;
  answer(taskId: string, text: string): Promise<string>;
  open(taskId: string, focus: TaskNotice["focus"]): Promise<void>;
  land(taskId: string): Promise<string>;
  restart(): Promise<void>;
  dismiss(id: string): Promise<void>;
  resize(height: number): void;
};

declare global {
  interface Window {
    mascot?: MascotBridge;
  }
}
