import { useMemo } from "react";
import { highlightCode, highlightDiff, type SyntaxKind } from "./syntax";

const tokenClass = (kind?: SyntaxKind) => (kind ? `syntax-${kind}` : undefined);

export function SyntaxHighlightedCode({
  text,
  path,
  diff = false,
}: {
  text: string;
  path: string;
  diff?: boolean;
}) {
  const lines = useMemo(
    () => (diff ? highlightDiff(text, path) : highlightCode(text, path)),
    [diff, path, text],
  );

  return (
    <pre className={`file-code ${diff ? "diff-code" : ""}`}>
      {lines.map((line, index) => (
        <div
          key={index}
          className={line.diffKind ? `diff-${line.diffKind}` : undefined}
        >
          <span className="line-number">{index + 1}</span>
          <code>
            {line.prefix && <span className="diff-prefix">{line.prefix}</span>}
            {line.tokens.length
              ? line.tokens.map((token, tokenIndex) => (
                  <span
                    className={tokenClass(token.kind)}
                    key={`${tokenIndex}-${token.text}`}
                  >
                    {token.text}
                  </span>
                ))
              : " "}
          </code>
        </div>
      ))}
    </pre>
  );
}
