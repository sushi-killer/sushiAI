import type { ReactNode } from "react";

/** A setting: what it is and what it does on the left, its control on the
 * right. The control drops under the text when the column is narrow. */
export function SettingRow({
  title,
  detail,
  children,
}: {
  title: string;
  detail: string;
  children: ReactNode;
}) {
  return (
    <div className="ui-setting">
      <span className="ui-setting-text">
        <strong>{title}</strong>
        <small>{detail}</small>
      </span>
      <span className="ui-setting-control">{children}</span>
    </div>
  );
}
