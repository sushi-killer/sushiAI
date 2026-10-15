import { usePageState } from "./pageState.ts";
import { Chip, Composer, MetaLine } from "../ui/index.ts";

/** The starting points of R1: they fill the composer, nothing more. */
export const RECIPES: { id: string; label: string; text: string }[] = [
  {
    id: "report",
    label: "Weekly report",
    text: "Every Monday at 9, write a short report of what changed in my-app last week and what is open.",
  },
  {
    id: "intake",
    label: "Task intake",
    text: "Every weekday at 9, read new issues of my-app, sort them and list the ones that need a decision.",
  },
  {
    id: "watch",
    label: "Watch a page",
    text: "Every hour, check that the build of my-app is green and tell me when it breaks.",
  },
  {
    id: "digest",
    label: "Morning digest",
    text: "Every weekday at 8, summarise yesterday's commits of my-app in five lines.",
  },
  {
    id: "cleanup",
    label: "Cleanup",
    text: "Every Friday, list branches of my-app that are merged and can be deleted.",
  },
];

/** R1: describe a routine in a sentence. The builder agent makes it. */
export function RoutineStart({
  busy,
  error,
  initial = "",
  title = "What should run on its own?",
  onBuild,
}: {
  busy: boolean;
  error: string;
  initial?: string;
  title?: string;
  onBuild(text: string): void;
}) {
  const [text, setText] = usePageState(`start-draft:${initial}`, initial);
  return (
    <div className="rt-start">
      <h2>{title}</h2>
      <MetaLine>
        Describe it once. The routine builder sets it up and shows a dry run
        before anything is written.
      </MetaLine>
      <Composer
        value={text}
        label="Describe the routine"
        placeholder="Every Friday collect closed Asana tasks and post a summary to #mobile"
        submitLabel="Build"
        autoFocus
        busy={busy}
        onChange={setText}
        onSubmit={onBuild}
      />
      {error && (
        <p role="alert" className="rt-error">
          {error}
        </p>
      )}
      <div className="rt-recipes" role="group" aria-label="Recipes">
        <span className="rt-lead">Or start from</span>
        {RECIPES.map((recipe) => (
          <Chip
            key={recipe.id}
            disabled={busy}
            onClick={() => setText(recipe.text)}
          >
            {recipe.label}
          </Chip>
        ))}
      </div>
    </div>
  );
}
