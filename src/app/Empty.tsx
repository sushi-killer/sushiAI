import { LayoutGrid, Plus } from "lucide-react";
import type React from "react";

export function LayersIcon() {
  return <LayoutGrid size={19} />;
}
export function Empty({
  icon,
  title,
  text,
  action,
  onAction,
}: {
  icon: React.ReactNode;
  title: string;
  text: string;
  action?: string;
  onAction?(): void;
}) {
  return (
    <div className="empty-state">
      {icon}
      <h2>{title}</h2>
      <p>{text}</p>
      {action && (
        <button className="primary" onClick={onAction}>
          <Plus size={14} />
          {action}
        </button>
      )}
    </div>
  );
}
