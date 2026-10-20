import { useEffect, useState } from "react";
import { Tag } from "../ui";
import { onProjectChange, projectNames, type NamesResult } from "./client";

/** Rows the Vault module adds to the Variables table: the names it provides
 * for the project, never the values. */
export function VaultEnvRows({
  projectId,
  listed,
}: {
  projectId: string;
  listed: readonly string[];
}) {
  const [result, setResult] = useState<NamesResult | null>(null);
  const [version, setVersion] = useState(0);

  useEffect(() => onProjectChange(() => setVersion((n) => n + 1)), []);

  useEffect(() => {
    let live = true;
    setResult(null);
    if (window.bridge)
      projectNames(projectId)
        .then((next) => live && setResult(next))
        .catch((reason) => {
          if (live)
            setResult({
              ok: false,
              reason: reason instanceof Error ? reason.message : String(reason),
            });
        });
    return () => {
      live = false;
    };
  }, [projectId, version]);

  if (!result) return null;
  if (!result.ok)
    return (
      <div className="pd-row provided vault-env-failed" role="row">
        <span />
        <span className="pd-cell pd-muted vault-env-reason" role="alert">
          <Tag tone="warning" dot={false}>
            {result.reason}
          </Tag>
        </span>
      </div>
    );
  return (
    <>
      {result.names.map((name) => (
        <div
          className={`pd-row provided vault-env-row${listed.includes(name) ? " overridden" : ""}`}
          role="row"
          key={name}
        >
          <span className="pd-cell lock" role="cell" />
          <span className="pd-cell pd-mono key vault-env-name" role="cell">
            {name}
            {listed.includes(name) ? (
              <span title="A variable entered here wins over Vault">
                <Tag tone="neutral" dot={false}>
                  overridden
                </Tag>
              </span>
            ) : (
              <Tag tone="info" dot={false}>
                Vault
              </Tag>
            )}
          </span>
          <span className="pd-cell pd-mono pd-faint" role="cell">
            hidden
          </span>
          <span className="pd-cell pd-muted" role="cell">
            all
          </span>
          <span className="pd-cell pd-faint" role="cell">
            This Mac
          </span>
        </div>
      ))}
    </>
  );
}
