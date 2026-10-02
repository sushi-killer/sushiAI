import { isValidElement, memo, type ReactElement, type ReactNode } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { SyntaxHighlightedCode } from "../../SyntaxHighlightedCode.tsx";

type CodeProps = { className?: string; children?: ReactNode };

/** Renders Markdown for the Preview. Raw HTML stays off, links open in the
 * system browser, fenced code gets the app's own highlighter. */
export const MarkdownDoc = memo(function MarkdownDoc({
  body,
}: {
  body: string;
}) {
  return (
    <Markdown
      remarkPlugins={[remarkGfm]}
      skipHtml
      components={{
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
          const language = /language-([\w-]+)/.exec(
            code?.props.className || "",
          )?.[1];
          const text = String(code?.props.children ?? "").replace(/\n$/, "");
          return (
            <div className="pv-code">
              <SyntaxHighlightedCode
                text={text}
                path={`code.${language || "txt"}`}
              />
            </div>
          );
        },
      }}
    >
      {body}
    </Markdown>
  );
});
