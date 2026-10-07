import qrcode from "qrcode-generator";

// Payloads are text, often with non-Latin characters; the library's default
// byte encoder would mangle them.
qrcode.stringToBytes = (text) => Array.from(new TextEncoder().encode(text));

/** The dark modules of a QR code for `text`, row by row. `null` when the text
 * does not fit the largest code. */
export function qrMatrix(text: string): boolean[][] | null {
  try {
    const code = qrcode(0, "M");
    code.addData(text, "Byte");
    code.make();
    const size = code.getModuleCount();
    return Array.from({ length: size }, (_, row) =>
      Array.from({ length: size }, (_, col) => code.isDark(row, col)),
    );
  } catch {
    return null;
  }
}

/** Quiet zone around the code, in modules, as the QR standard asks. */
export const QR_QUIET = 4;

/** One SVG path for the matrix: a run of dark modules in a row is one `h`
 * segment, so the markup stays small. Coordinates include the quiet zone. */
export function qrPath(matrix: boolean[][]): string {
  const parts: string[] = [];
  matrix.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      if (!row[x]) {
        x += 1;
        continue;
      }
      const start = x;
      while (x < row.length && row[x]) x += 1;
      parts.push(
        `M${start + QR_QUIET} ${y + QR_QUIET}h${x - start}v1h-${x - start}z`,
      );
    }
  });
  return parts.join("");
}
