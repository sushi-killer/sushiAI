import type { ReactNode } from "react";

/** The frame every full-page view sits in: eyebrow, title, description and a
 * row of controls on the right. Core sections and extension pages share it, so
 * a page an extension contributes is laid out by the host rather than by the
 * manifest. */
export function PageFrame({
  eyebrow,
  title,
  description,
  actions,
  children,
}: {
  eyebrow: string;
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="section-page">
      <div className="page-eyebrow">{eyebrow}</div>
      <div className="page-heading">
        <div>
          <h1>{title}</h1>
          {description && <p>{description}</p>}
        </div>
        {actions && <div className="page-heading-actions">{actions}</div>}
      </div>
      {children}
    </div>
  );
}
