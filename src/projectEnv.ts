export type ParsedEnvEntry = { name: string; value: string };

function closingQuote(value: string, quote: string): number {
  for (let index = 0; index < value.length; index++) {
    if (quote === '"' && value[index] === "\\") {
      index++;
      continue;
    }
    if (value[index] === quote) return index;
  }
  return -1;
}

export function parseEnv(source: string): ParsedEnvEntry[] {
  const entries: ParsedEnvEntry[] = [];
  let index = 0;
  while (index < source.length) {
    while (index < source.length && /\s/.test(source[index])) index++;
    if (index >= source.length) break;
    if (source[index] === "#") {
      while (index < source.length && source[index] !== "\n") index++;
      continue;
    }
    const lineEnd = source.indexOf("\n", index);
    const end = lineEnd < 0 ? source.length : lineEnd;
    let line = source.slice(index, end).replace(/\r$/, "").trim();
    index = lineEnd < 0 ? source.length : lineEnd + 1;
    line = line.replace(/^export\s+/, "");
    const match = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*/.exec(line);
    if (!match) continue;
    const name = match[1];
    let value = line.slice(match[0].length);
    const quote = value[0];
    if (quote === "'" || quote === '"' || quote === "`") {
      value = value.slice(1);
      let closing = closingQuote(value, quote);
      while (closing < 0 && index < source.length) {
        const nextEnd = source.indexOf("\n", index);
        const next =
          nextEnd < 0 ? source.slice(index) : source.slice(index, nextEnd);
        value += `\n${next.replace(/\r$/, "")}`;
        index = nextEnd < 0 ? source.length : nextEnd + 1;
        closing = closingQuote(value, quote);
      }
      if (closing >= 0) value = value.slice(0, closing);
      if (quote === '"')
        value = value.replace(/\\(["\\$`])/g, "$1").replace(/\\n/g, "\n");
    } else {
      value = value.replace(/\s+#.*$/, "").trimEnd();
    }
    entries.push({ name, value });
  }
  return entries;
}

export function isSecretEnvName(name: string): boolean {
  return /_(TOKEN|KEY|SECRET)$/.test(name);
}

export function displayedEnvValue(
  value: string,
  secret: boolean,
  hint = "",
): string {
  const suffix = hint.replace(/^•+/, "");
  return secret ? `••••••••${suffix ? ` ${suffix}` : ""}` : value;
}
