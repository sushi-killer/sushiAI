import type { ReactNode } from "react";

/** The page every Project settings tab shares: a fixed title and subtitle,
 * then a body that scrolls. The dialog's own close button sits at the right of
 * the title row. */
export function ProjectPage({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle: string;
  children: ReactNode;
}) {
  return (
    <>
      <header className="pd-title">
        <h2>{title}</h2>
        <p>{subtitle}</p>
      </header>
      <div className="pd-body">{children}</div>
    </>
  );
}
