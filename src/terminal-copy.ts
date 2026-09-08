// Herdr's rendered frames no longer carry the original soft-wrap flags.
// Join only prose continuations; preserve structural and ambiguous line breaks.
export function cleanTerminalCopy(
  text: string,
  columns?: number,
  contextLine = "",
) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const result: string[] = [];
  let fenced = false;
  let previous = "";
  let claudeParagraph = /^ {0,3}⏺\s+/.test(contextLine);
  let firstLine = true;
  const structure = (line: string) =>
    /^(?:\s{4}|\t|\s*(?:```|~~~|[|│┃┌└├╭╰]|#{1,6}\s|[-*+]\s|\d+[.)]\s|[⏺❯>]|(?:const|let|var|def|class|import|from|return|if|for|while|function)\b))/.test(
      line,
    ) ||
    /[{};]\s*$/.test(line) ||
    /(?:=>|:=|\w\s*=\s*[^=])/.test(line);
  for (let line of lines) {
    // Some clipboard/Markdown paths encode indentation as HTML space entities.
    line = line.replace(
      /^(?:(?:&#x20;|&#32;|&nbsp;|\u00a0)|[ \t])+/i,
      (prefix) => prefix.replace(/&#x20;|&#32;|&nbsp;|\u00a0/gi, " "),
    );
    const raw = line;
    line = line.replace(/[\t ]+$/g, "");
    if (/^\s*(?:```|~~~)/.test(line)) {
      fenced = !fenced;
      claudeParagraph = false;
    }
    const marker = /^ {0,3}⏺\s+/.test(previous);
    const prosePrevious = previous.replace(/^ {0,3}⏺\s+/, "");
    const nearEdge =
      columns !== undefined &&
      Array.from(previous).length >= Math.max(40, columns - 24);
    const continuation = /^ {0,3}\S/.test(line);
    const prose = /\p{L}/u.test(line) && prosePrevious.split(/\s+/).length >= 5;
    const join: boolean =
      !fenced &&
      !!previous.trim() &&
      !!line.trim() &&
      !structure(prosePrevious) &&
      !structure(line) &&
      prose &&
      ((nearEdge && continuation) ||
        ((marker || claudeParagraph) && previous.length >= 60 && continuation));
    if (!fenced && !structure(prosePrevious) && !structure(line))
      line = line.replace(/(\S)[ \t]{2,}(?=\S)/g, "$1 ");
    else if (!fenced && /^ {0,3}⏺\s+/.test(line))
      line = line.replace(/(\S)[ \t]{2,}(?=\S)/g, "$1 ");
    if (join) result[result.length - 1] += " " + line.trimStart();
    else result.push(line);
    claudeParagraph =
      !!line.trim() &&
      (join
        ? marker || claudeParagraph
        : /^ {0,3}⏺\s+/.test(line) || (firstLine && claudeParagraph));
    firstLine = false;
    previous = raw.replace(/[\t ]+$/g, "");
  }
  return result.join("\n");
}
