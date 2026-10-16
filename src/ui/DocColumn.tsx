import type { ReactNode } from "react";

/** A reading column: no cards, a head with a kind badge, a line of facts and
 * the tools, then the document. */
export function DocColumn({
  kind,
  meta,
  tools,
  children,
}: {
  kind: string;
  meta: string;
  tools?: ReactNode;
  children: ReactNode;
}) {
  return (
    <article className="ui-doc">
      <div className="ui-doc-head">
        <span className="ui-doc-meta">
          <span className="ui-doc-kind">{kind}</span>
          <span className="ui-doc-facts" title={meta}>
            {meta}
          </span>
        </span>
        {tools && <span className="ui-doc-tools">{tools}</span>}
      </div>
      {children}
    </article>
  );
}

/** The body of a rendered markdown document: headings that never outgrow the
 * page title, tight lists, inline code. */
export function DocBody({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={`ui-doc-body ${className}`.trim()}>{children}</div>;
}
