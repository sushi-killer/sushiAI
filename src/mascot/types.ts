/** One button a notice offers; `id` goes back to its source untouched. */
export type MascotAction = {
  id: string;
  label: string;
  emphasis?: "primary" | "ghost";
  icon?: "check" | "refresh";
};

/** One queued notice as electron/mascot.cjs publishes it. */
export type MascotNotice = {
  /** Source and key together: the id every bridge call uses. */
  id: string;
  source: string;
  key: string;
  kind: "input" | "done" | "failed" | "info";
  title: string;
  body: string;
  /** The tag text; defaults from `kind`. */
  label?: string;
  /** Where it comes from, shown beside the tag with the age of `at`. */
  header?: string;
  /** Epoch ms the thing happened. */
  at?: number;
  /** A second line (parts joined with a dot) that replaces the body or title. */
  meta?: string[];
  choices?: string[];
  /** Shows a reply field. */
  reply?: boolean;
  actions?: MascotAction[];
  sticky?: boolean;
  /** Epoch ms the notice leaves at; null while it waits for the user. */
  expiresAt: number | null;
  /** The source's message once an action succeeded; the bubble shows it. */
  confirmed?: string;
};

export type MascotBridge = {
  onNotices(callback: (notices: MascotNotice[]) => void): () => void;
  /** Runs an action of a queued notice; "reply" sends `text`. */
  act(id: string, actionId: string, text?: string): Promise<string | undefined>;
  /** ⌥Space: flip between the bubble stack and the pill. */
  onToggle(callback: () => void): () => void;
  /** True while a fullscreen app or a slideshow owns the primary display. */
  onPresenting(callback: (presenting: boolean) => void): () => void;
  /** Answer all in Inbox: brings the main window up on the Inbox. */
  openInbox(): Promise<void>;
  /** Lets the reply field take typing after ⌥Space expands the stack. */
  focus(): Promise<void>;
  dismiss(id: string): Promise<void>;
  resize(height: number): void;
};

declare global {
  interface Window {
    mascot?: MascotBridge;
  }
}
