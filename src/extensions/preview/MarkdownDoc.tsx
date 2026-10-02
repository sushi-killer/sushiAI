import { CircleCheck } from "lucide-react";
import {
  isValidElement,
  memo,
  useMemo,
  type ReactElement,
  type ReactNode,
} from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { SyntaxHighlightedCode } from "../../SyntaxHighlightedCode.tsx";
import { planSegments } from "./artifact.ts";

type CodeProps = { className?: string; children?: ReactNode };

const BASE: Components = {
  a: ({ href, children }) => (
    <a
      href={href}
      onClick={(event) => {
        event.preventDefault();
        if (href && /^https?:/i.test(href))
          void window.bridge?.agentOpenExternal(href).catch(() => {});
      }}
    >
      {children}
    </a>
  ),
  pre: ({ children }) => {
    const code = isValidElement(children)
      ? (children as ReactElement<CodeProps>)
      : null;
    const language = /language-([\w-]+)/.exec(code?.props.className || "")?.[1];
    const text = String(code?.props.children ?? "").replace(/\n$/, "");
    return (
      <div className="pv-code">
        <SyntaxHighlightedCode text={text} path={`code.${language || "txt"}`} />
      </div>
    );
  },
};

/** Under `## Done when` a plain list item gets a check instead of a bullet. */
const DONE: Components = {
  ...BASE,
  li: ({ children, className }) =>
    className?.includes("task-list-item") ? (
      <li className={className}>{children}</li>
    ) : (
      <li>
        <CircleCheck size={13} aria-hidden />
        <span>{children}</span>
      </li>
    ),
};

const render = (text: string, components: Components) => (
  <Markdown remarkPlugins={[remarkGfm]} skipHtml components={components}>
    {text}
  </Markdown>
);

/** Renders Markdown for the Preview. Raw HTML stays off, links open in the
 * system browser, fenced code gets the app's own highlighter. In a plan the
 * Goal sits in a card and the Done when items carry checks. */
export const MarkdownDoc = memo(function MarkdownDoc({
  body,
  plan,
}: {
  body: string;
  plan?: boolean;
}) {
  const segments = useMemo(
    () => (plan ? planSegments(body) : []),
    [plan, body],
  );
  if (!plan) return render(body, BASE);
  return (
    <>
      {segments.map((segment, index) =>
        segment.kind === "goal" ? (
          <section key={index} className="pv-goal">
            <span className="pv-eyebrow">Goal</span>
            {render(segment.text, BASE)}
          </section>
        ) : segment.kind === "done" ? (
          <div key={index} className="pv-done">
            {render(segment.text, DONE)}
          </div>
        ) : (
          <div key={index}>{render(segment.text, BASE)}</div>
        ),
      )}
    </>
  );
});
