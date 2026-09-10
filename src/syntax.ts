export type SyntaxKind =
  | "comment"
  | "string"
  | "number"
  | "keyword"
  | "constant"
  | "type"
  | "function"
  | "property"
  | "decorator"
  | "operator"
  | "punctuation"
  | "tag"
  | "attribute"
  | "heading";

export type SyntaxToken = { text: string; kind?: SyntaxKind };

type Language =
  | "bash"
  | "c"
  | "cpp"
  | "csharp"
  | "css"
  | "go"
  | "html"
  | "java"
  | "javascript"
  | "json"
  | "kotlin"
  | "markdown"
  | "php"
  | "python"
  | "ruby"
  | "rust"
  | "sql"
  | "swift"
  | "toml"
  | "typescript"
  | "yaml"
  | "plain";

type HighlightState = {
  blockComment: boolean;
  stringQuote?: string;
};

export type DiffLineKind =
  "addition" | "deletion" | "hunk" | "meta" | "context";
export type HighlightedLine = {
  tokens: SyntaxToken[];
  diffKind?: DiffLineKind;
  prefix?: string;
};

const EXTENSIONS: Record<string, Language> = {
  ".c": "c",
  ".cc": "cpp",
  ".cpp": "cpp",
  ".cxx": "cpp",
  ".cs": "csharp",
  ".css": "css",
  ".go": "go",
  ".htm": "html",
  ".html": "html",
  ".java": "java",
  ".js": "javascript",
  ".jsx": "javascript",
  ".json": "json",
  ".jsonc": "json",
  ".kt": "kotlin",
  ".kts": "kotlin",
  ".md": "markdown",
  ".markdown": "markdown",
  ".php": "php",
  ".py": "python",
  ".pyw": "python",
  ".rb": "ruby",
  ".rs": "rust",
  ".scss": "css",
  ".sh": "bash",
  ".sql": "sql",
  ".swift": "swift",
  ".toml": "toml",
  ".ts": "typescript",
  ".tsx": "typescript",
  ".xml": "html",
  ".yaml": "yaml",
  ".yml": "yaml",
};

const KEYWORDS = new Set([
  "abstract",
  "and",
  "as",
  "assert",
  "async",
  "await",
  "break",
  "case",
  "catch",
  "class",
  "const",
  "continue",
  "default",
  "def",
  "delete",
  "do",
  "elif",
  "else",
  "enum",
  "except",
  "export",
  "extends",
  "final",
  "finally",
  "for",
  "from",
  "fun",
  "function",
  "get",
  "global",
  "if",
  "implements",
  "import",
  "in",
  "instanceof",
  "interface",
  "is",
  "lambda",
  "let",
  "match",
  "namespace",
  "new",
  "nonlocal",
  "not",
  "of",
  "operator",
  "or",
  "override",
  "package",
  "pass",
  "private",
  "protected",
  "public",
  "raise",
  "readonly",
  "return",
  "select",
  "set",
  "static",
  "struct",
  "super",
  "switch",
  "throw",
  "throws",
  "trait",
  "try",
  "type",
  "typeof",
  "union",
  "val",
  "var",
  "where",
  "while",
  "with",
  "yield",
]);

const TYPES = new Set([
  "any",
  "array",
  "bool",
  "boolean",
  "char",
  "date",
  "dict",
  "double",
  "error",
  "float",
  "int",
  "list",
  "map",
  "never",
  "number",
  "object",
  "promise",
  "rune",
  "set",
  "string",
  "str",
  "uint",
  "unknown",
  "void",
]);

const CONSTANTS = new Set([
  "false",
  "infinity",
  "nan",
  "nil",
  "none",
  "null",
  "true",
  "undefined",
]);

const WORD_RE = /[A-Za-z_$]/;
const WORD_PART_RE = /[A-Za-z0-9_$]/;

export function languageFromPath(path: string): Language {
  const name = path.split(/[\\/]/).pop()?.toLowerCase() || "";
  if (name === "dockerfile" || name.startsWith("dockerfile.")) return "bash";
  if (name === "makefile" || name === "gnumakefile") return "bash";
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? EXTENSIONS[name.slice(dot)] || "plain" : "plain";
}

function push(tokens: SyntaxToken[], text: string, kind?: SyntaxKind) {
  if (!text) return;
  const previous = tokens[tokens.length - 1];
  if (previous && previous.kind === kind) previous.text += text;
  else tokens.push({ text, ...(kind ? { kind } : {}) });
}

function isMarkup(language: Language) {
  return ["html", "javascript", "typescript"].includes(language);
}

function commentStart(line: string, index: number, language: Language) {
  if (line.startsWith("//", index) && language !== "css") return 2;
  if (line.startsWith("--", index) && language === "sql") return 2;
  if (
    line[index] === "#" &&
    ["bash", "python", "ruby", "yaml", "toml"].includes(language)
  )
    return 1;
  return 0;
}

function quotedEnd(line: string, start: number, quote: string) {
  for (let index = start + 1; index < line.length; index += 1) {
    if (line[index] === "\\") {
      index += 1;
      continue;
    }
    if (line[index] === quote) return index + 1;
  }
  return line.length;
}

function markupTagEnd(line: string, start: number) {
  if (!/^<\/?[A-Za-z!?]/.test(line.slice(start))) return -1;
  let quote = "";
  for (let index = start + 1; index < line.length; index += 1) {
    const character = line[index];
    if (quote) {
      if (character === "\\") index += 1;
      else if (character === quote) quote = "";
    } else if (character === '"' || character === "'") quote = character;
    else if (character === ">") return index + 1;
  }
  return -1;
}

function markupTokens(tag: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  let index = 0;
  while (index < tag.length) {
    if (tag.startsWith("<!--", index)) {
      push(tokens, tag.slice(index), "comment");
      break;
    }
    if (tag.startsWith("</", index)) {
      push(tokens, "</", "punctuation");
      index += 2;
      continue;
    }
    if (tag[index] === "<" || tag[index] === ">" || tag[index] === "/") {
      push(tokens, tag[index], "punctuation");
      index += 1;
      continue;
    }
    if (tag[index] === '"' || tag[index] === "'") {
      const end = quotedEnd(tag, index, tag[index]);
      push(tokens, tag.slice(index, end), "string");
      index = end;
      continue;
    }
    if (WORD_RE.test(tag[index])) {
      const start = index;
      index += 1;
      while (index < tag.length && /[A-Za-z0-9:_.-]/.test(tag[index]))
        index += 1;
      const word = tag.slice(start, index);
      let lookahead = index;
      while (/\s/.test(tag[lookahead] || "")) lookahead += 1;
      push(
        tokens,
        word,
        lookahead < tag.length && tag[lookahead] === "=" ? "attribute" : "tag",
      );
      continue;
    }
    if (tag[index] === "=") push(tokens, tag[index], "operator");
    else if (/[{}()[\],;:.]/.test(tag[index]))
      push(tokens, tag[index], "punctuation");
    else push(tokens, tag[index]);
    index += 1;
  }
  return tokens;
}

function classifyWord(
  word: string,
  line: string,
  end: number,
  language: Language,
): SyntaxKind | undefined {
  const lower = word.toLowerCase();
  if (CONSTANTS.has(lower)) return "constant";
  if (KEYWORDS.has(lower)) return "keyword";
  if (TYPES.has(lower) || (language === "typescript" && /^[A-Z]/.test(word)))
    return "type";

  let lookahead = end;
  while (/\s/.test(line[lookahead] || "")) lookahead += 1;
  if (line[lookahead] === "(") return "function";
  if (line[lookahead] === ":") return "property";
  return undefined;
}

function markdownTokens(line: string) {
  const heading = line.match(/^(\s*)(#{1,6}\s+.*)$/);
  if (heading)
    return [
      { text: heading[1] },
      { text: heading[2], kind: "heading" as const },
    ];
  if (/^\s*(```|~~~)/.test(line))
    return [{ text: line, kind: "keyword" as const }];
  return [{ text: line }];
}

function yamlScalarKind(value: string): SyntaxKind | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  if (/^(true|false|null|~)$/i.test(trimmed)) return "constant";
  if (/^-?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(trimmed))
    return "number";
  return "string";
}

function frontmatterTokens(line: string): SyntaxToken[] {
  if (line.trim() === "---") return [{ text: line, kind: "punctuation" }];

  const match = line.match(
    /^(\s*)([-]?\s*)([A-Za-z_][\w-]*)(\s*)(:)(\s*)(.*)$/,
  );
  if (!match) return [{ text: line }];

  const [, indent, listMarker, key, spacing, colon, afterColon, value] = match;
  const tokens: SyntaxToken[] = [];
  push(tokens, indent);
  push(tokens, listMarker, listMarker.trim() ? "punctuation" : undefined);
  push(tokens, key, "property");
  push(tokens, spacing);
  push(tokens, colon, "punctuation");
  push(tokens, afterColon);
  push(tokens, value, yamlScalarKind(value));
  return tokens;
}

function markdownCode(text: string): HighlightedLine[] {
  const lines = text.split("\n");
  const hasFrontmatter = lines[0]?.trim() === "---";
  let inFrontmatter = hasFrontmatter;

  return lines.map((line, index) => {
    if (inFrontmatter) {
      const tokens = frontmatterTokens(line);
      if (index > 0 && line.trim() === "---") inFrontmatter = false;
      return { tokens };
    }
    return { tokens: markdownTokens(line) };
  });
}

export function highlightLine(
  line: string,
  language: Language,
  state: HighlightState = { blockComment: false },
): SyntaxToken[] {
  if (language === "plain") return [{ text: line }];
  if (language === "markdown") return markdownTokens(line);

  const tokens: SyntaxToken[] = [];
  let index = 0;
  while (index < line.length) {
    if (state.blockComment) {
      const end = line.indexOf("*/", index);
      if (end < 0) {
        push(tokens, line.slice(index), "comment");
        return tokens;
      }
      push(tokens, line.slice(index, end + 2), "comment");
      state.blockComment = false;
      index = end + 2;
      continue;
    }
    if (state.stringQuote) {
      const end = quotedEnd(line, -1, state.stringQuote);
      push(tokens, line.slice(0, end), "string");
      if (end === line.length && line.at(-1) !== state.stringQuote)
        return tokens;
      state.stringQuote = undefined;
      index = end;
      continue;
    }

    if (isMarkup(language) && line[index] === "<") {
      const end = markupTagEnd(line, index);
      if (end >= 0) {
        for (const token of markupTokens(line.slice(index, end)))
          push(tokens, token.text, token.kind);
        index = end;
        continue;
      }
    }
    if (line.startsWith("<!--", index)) {
      const end = line.indexOf("-->", index + 4);
      push(
        tokens,
        line.slice(index, end < 0 ? line.length : end + 3),
        "comment",
      );
      index = end < 0 ? line.length : end + 3;
      continue;
    }
    const commentLength = commentStart(line, index, language);
    if (
      commentLength ||
      (line.startsWith("/*", index) && language !== "json")
    ) {
      if (commentLength) {
        push(tokens, line.slice(index), "comment");
        break;
      }
      const end = line.indexOf("*/", index + 2);
      if (end < 0) {
        push(tokens, line.slice(index), "comment");
        state.blockComment = true;
        break;
      }
      push(tokens, line.slice(index, end + 2), "comment");
      index = end + 2;
      continue;
    }
    if (line[index] === "@") {
      const start = index;
      index += 1;
      while (index < line.length && WORD_PART_RE.test(line[index])) index += 1;
      push(tokens, line.slice(start, index), "decorator");
      continue;
    }
    if (line[index] === '"' || line[index] === "'" || line[index] === "`") {
      const quote = line[index];
      const end = quotedEnd(line, index, quote);
      push(tokens, line.slice(index, end), "string");
      if (end === line.length && line.at(-1) !== quote)
        state.stringQuote = quote;
      index = end;
      continue;
    }
    if (WORD_RE.test(line[index])) {
      const start = index;
      index += 1;
      while (index < line.length && WORD_PART_RE.test(line[index])) index += 1;
      const word = line.slice(start, index);
      push(tokens, word, classifyWord(word, line, index, language));
      continue;
    }
    if (
      /\d/.test(line[index]) &&
      (index === 0 || !WORD_PART_RE.test(line[index - 1]))
    ) {
      const start = index;
      index += 1;
      while (index < line.length && /[A-Za-z0-9._]/.test(line[index]))
        index += 1;
      push(tokens, line.slice(start, index), "number");
      continue;
    }
    if (/[+*/%=!<>?&|^-]/.test(line[index])) {
      const start = index;
      index += 1;
      while (index < line.length && /[+*/%=!<>?&|^-]/.test(line[index]))
        index += 1;
      push(tokens, line.slice(start, index), "operator");
      continue;
    }
    if (/[{}()[\];,.:]/.test(line[index]))
      push(tokens, line[index], "punctuation");
    else push(tokens, line[index]);
    index += 1;
  }
  return tokens;
}

function mergeTokens(tokens: SyntaxToken[], next: SyntaxToken[]) {
  for (const token of next) push(tokens, token.text, token.kind);
}

function diffParts(line: string): {
  kind: DiffLineKind;
  prefix?: string;
  source: string;
} {
  if (line.startsWith("@@")) return { kind: "hunk", source: line };
  if (
    line.startsWith("diff ") ||
    line.startsWith("index ") ||
    line.startsWith("--- ") ||
    line.startsWith("+++ ") ||
    line.startsWith("\\ No newline")
  )
    return { kind: "meta", source: line };
  if (line.startsWith("+"))
    return { kind: "addition", prefix: "+", source: line.slice(1) };
  if (line.startsWith("-"))
    return { kind: "deletion", prefix: "-", source: line.slice(1) };
  if (line.startsWith(" "))
    return { kind: "context", prefix: " ", source: line.slice(1) };
  return { kind: "meta", source: line };
}

export function highlightCode(text: string, path: string): HighlightedLine[] {
  const language = languageFromPath(path);
  if (language === "markdown") return markdownCode(text);
  const state: HighlightState = { blockComment: false };
  return text
    .split("\n")
    .map((line) => ({ tokens: highlightLine(line, language, state) }));
}

export function highlightDiff(text: string, path: string): HighlightedLine[] {
  const language = languageFromPath(path);
  const state: HighlightState = { blockComment: false };
  return text.split("\n").map((line) => {
    const part = diffParts(line);
    const tokens =
      part.kind === "addition" ||
      part.kind === "deletion" ||
      part.kind === "context"
        ? highlightLine(part.source, language, state)
        : [{ text: part.source }];
    return { tokens, diffKind: part.kind, prefix: part.prefix };
  });
}
