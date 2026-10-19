// What size a block or a note needs for its text. Pure and deterministic: the
// width of a character is an estimate that errs wide, so a block sized by it
// never clips, in the app and in a node test alike.

// Advance widths in em of the system sans (SF Pro), rounded up a little.
const LOWER = [
  0.54, 0.59, 0.53, 0.59, 0.55, 0.33, 0.59, 0.58, 0.26, 0.26, 0.53, 0.26, 0.88,
  0.58, 0.59, 0.59, 0.59, 0.36, 0.51, 0.34, 0.58, 0.51, 0.76, 0.51, 0.51, 0.49,
];
const SAFETY = 1.04;

function charEm(ch: string): number {
  const code = ch.codePointAt(0) as number;
  if (code >= 0x2e80) return 1.05;
  if (ch >= "a" && ch <= "z") return LOWER[code - 97];
  if (ch >= "A" && ch <= "Z")
    return ch === "I" ? 0.3 : "MW".includes(ch) ? 0.95 : 0.68;
  if (ch >= "0" && ch <= "9") return 0.62;
  if (ch === " ") return 0.28;
  if (".,:;'|!".includes(ch)) return 0.28;
  if ('()[]/\\-"`'.includes(ch)) return 0.36;
  if ("@%".includes(ch)) return 0.95;
  return code < 128 ? 0.6 : 0.64;
}

/** The width in px `text` takes on one line at font size `px`. */
export function textWidth(text: string, px: number, bold = false): number {
  let em = 0;
  for (const ch of text) em += charEm(ch);
  return Math.ceil(em * px * (bold ? 1.04 : 1) * SAFETY);
}

const paragraphs = (text: string, keepBreaks: boolean) =>
  keepBreaks ? text.split("\n") : [text.replace(/\s+/g, " ").trim()];

/** The width of the longest line `text` has before any wrapping. */
export function naturalWidth(
  text: string,
  px: number,
  bold = false,
  keepBreaks = false,
): number {
  return Math.max(
    0,
    ...paragraphs(text, keepBreaks).map((p) => textWidth(p, px, bold)),
  );
}

/** How many lines `text` takes in a column `avail` px wide: words wrap, a word
 * wider than the column breaks anywhere. */
export function lineCount(
  text: string,
  px: number,
  avail: number,
  bold = false,
  keepBreaks = false,
): number {
  const room = Math.max(avail, px);
  const space = textWidth(" ", px, bold);
  let lines = 0;
  for (const para of paragraphs(text, keepBreaks)) {
    let rows = 1;
    let line = "";
    // Width of the last fragment of a broken word, which the next word follows.
    let tail = 0;
    for (const word of para.split(/\s+/).filter(Boolean)) {
      const joined = line ? `${line} ${word}` : word;
      const lead = tail > 0 ? tail + space : 0;
      if (lead + textWidth(joined, px, bold) <= room) {
        line = joined;
        continue;
      }
      if (line || tail > 0) rows++;
      tail = 0;
      const w = textWidth(word, px, bold);
      // A word wider than the column breaks anywhere.
      if (w > room) {
        const extra = Math.ceil(w / room) - 1;
        rows += extra;
        tail = Math.max(1, w - extra * room);
        line = "";
      } else line = word;
    }
    lines += rows;
  }
  return Math.max(1, lines);
}

type Size = { w: number; h: number };

export const SHAPE_SIZES: Record<string, Size> = {
  box: { w: 170, h: 62 },
  pill: { w: 170, h: 44 },
  diamond: { w: 150, h: 90 },
  cylinder: { w: 170, h: 72 },
  person: { w: 120, h: 84 },
  circle: { w: 72, h: 72 },
};

const TITLE_PX = 12.5;
const TITLE_LH = 16;
const TECH_PX = 10.5;
const TECH_LH = 13;
const MAX_W = 320;
const MAX_FIT = 600;

/** The text of a block, and the chrome around it that sets its size. */
export type BlockText = {
  shape: string;
  title: string;
  tech: string;
  /** Width of the icon, caption and tag row; 0 when the block has none. */
  kindRow: number;
  /** The block draws an icon beside its title (a pill). */
  icon: boolean;
};

export const MIN_NODE_W = 60;
export const MIN_NODE_H = 32;

const even = (v: number) => Math.ceil(v / 2) * 2;

/** The size a block needs. Without a stored width its shape's own width grows
 * to the text, up to a maximum, then the text wraps. A stored width is kept
 * (not below the least a block needs) and the text wraps in it. The height
 * grows to hold every line and is not below a stored height. */
export function fitBlock(t: BlockText, min: Partial<Size> = {}): Size {
  const base = SHAPE_SIZES[t.shape] ?? SHAPE_SIZES.box;
  const minW = min.w ?? 0;
  const minH = min.h ?? 0;
  const title = (avail: number) =>
    lineCount(t.title, TITLE_PX, avail, true) * TITLE_LH;
  const tech = (avail: number) =>
    t.tech ? lineCount(t.tech, TECH_PX, avail) * TECH_LH : 0;

  if (t.shape === "diamond" || t.shape === "circle") {
    const diamond = t.shape === "diamond";
    const [px, lh, across, down] = diamond
      ? [12, 14, 0.6, 0.4]
      : [11, 13, 0.7, 0.7];
    const baseH = minH || base.h;
    const fixed = minW > 0;
    let w = Math.max(minW || base.w, MIN_NODE_W);
    let need = 0;
    for (; w <= MAX_FIT; w += diamond ? 10 : 8) {
      need = Math.ceil((lineCount(t.title, px, w * across, true) * lh) / down);
      if (fixed || need <= Math.max(baseH, diamond ? w * 0.75 : w) || w >= 300)
        break;
    }
    const roundH = diamond || minH || fixed ? 0 : w;
    return { w: even(w), h: even(Math.max(baseH, need, roundH)) };
  }

  const frames: Record<string, { padX: number; padY: number; maxW: number }> = {
    // Padding plus the 1px border, as the CSS has it.
    box: { padX: 22, padY: 17, maxW: MAX_W },
    // Padding 12 + 4, and the 8px arc under the text.
    cylinder: { padX: 20, padY: 24, maxW: MAX_W },
    pill: { padX: t.icon ? 48 : 30, padY: 16, maxW: MAX_W },
    person: { padX: 12, padY: 40, maxW: 240 },
  };
  const f = frames[t.shape] ?? frames.box;
  const natural = Math.max(
    naturalWidth(t.title, TITLE_PX, true),
    t.tech ? naturalWidth(t.tech, TECH_PX) : 0,
    t.kindRow,
  );
  const w = even(
    minW
      ? Math.max(minW, MIN_NODE_W, t.kindRow + f.padX)
      : Math.max(base.w, Math.min(f.maxW, natural + f.padX)),
  );
  const avail = w - f.padX;
  const gap = t.tech ? 3 : 0;
  const row = t.kindRow ? 16 : 0;
  const content = row + title(avail) + gap + tech(avail);
  return { w, h: even(Math.max(minH || base.h, content + f.padY)) };
}

export const NOTE_MAX_W = 280;
const NOTE_PX = 11.5;
const NOTE_LH = 15;
const NOTE_CHROME_X = 39;
const NOTE_CHROME_Y = 16;

/** The size a note needs: one line up to the maximum width, then it wraps. */
export function fitNote(text: string): Size {
  const natural = naturalWidth(text, NOTE_PX, false, true);
  const w = even(Math.min(NOTE_MAX_W, natural + NOTE_CHROME_X));
  const lines = lineCount(text, NOTE_PX, w - NOTE_CHROME_X, false, true);
  return { w, h: even(lines * NOTE_LH + NOTE_CHROME_Y) };
}
