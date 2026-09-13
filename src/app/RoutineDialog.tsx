import { uid } from "../layout.ts";
import type { Routine } from "../workspaceState.ts";

export function RoutineDialog({
  setRoutines,
  close,
}: {
  setRoutines(update: (items: Routine[]) => Routine[]): void;
  close(): void;
}) {
  return (
    <>
      <div className="dialog-eyebrow">A LITTLE LESS REPETITION</div>
      <h2>New routine</h2>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          setRoutines((items) => [
            ...items,
            {
              id: uid(),
              name: String(data.get("name")),
              command: String(data.get("command")),
            },
          ]);
          close();
        }}
      >
        <label>
          Name
          <input
            name="name"
            autoFocus
            required
            placeholder="Start development"
          />
        </label>
        <label>
          Command
          <input name="command" required placeholder="npm run dev" />
        </label>
        <p className="muted">
          Runs in the selected workspace when you press Play.
        </p>
        <button className="primary" type="submit">
          Save routine
        </button>
      </form>
    </>
  );
}
