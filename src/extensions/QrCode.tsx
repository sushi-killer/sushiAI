import { useMemo } from "react";
import { QR_QUIET, qrMatrix, qrPath } from "./qrMatrix.ts";

/** Draws text as a QR code. The host builds the SVG from the string; an
 * extension never supplies image bytes. */
export function QrCode({ text, label }: { text: string; label: string }) {
  const code = useMemo(() => {
    const matrix = qrMatrix(text);
    return matrix && { size: matrix.length, path: qrPath(matrix) };
  }, [text]);
  if (!code)
    return <span className="companion-qr-missing">QR unavailable</span>;
  const side = code.size + QR_QUIET * 2;
  return (
    <svg
      className="companion-qr"
      viewBox={`0 0 ${side} ${side}`}
      role="img"
      aria-label={label}
      shapeRendering="crispEdges"
    >
      <rect width={side} height={side} className="companion-qr-paper" />
      <path d={code.path} className="companion-qr-ink" />
    </svg>
  );
}
