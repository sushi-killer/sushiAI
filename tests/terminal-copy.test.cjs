const { test } = require("node:test");
const assert = require("node:assert/strict");
const library = import("../src/terminal-copy.ts");

test("checkpoint sentence copies consistently at different widths and without indentation", async () => {
  const { cleanTerminalCopy } = await library;
  const first = "⏺ Записано в PLAN-NEXT.md. Теперь запускаю батч-прогон по всем 5 чекпоинтам r1-retry (последовательная загрузка, но каждый dev-прогон идёт";
  const next = "с параллельными запросами вместо --workers 1).";
  for (const columns of [130, 180, 240]) {
    for (const padding of ["", " ", "  "])
      assert.equal(cleanTerminalCopy(first + "\n" + padding + next, columns), first + " " + next);
    const fragment = first.slice(first.indexOf("Теперь"));
    assert.equal(cleanTerminalCopy(fragment + "\n" + next, columns, first), fragment + " " + next);
  }
});

test("Claude prose loses terminal wraps and encoded continuation padding", async () => {
  const { cleanTerminalCopy } = await library;
  const first =
    "⏺ Теперь перезапускаю обучение раунда r1 с чекпоинтами (та же выборка, тот же сид — только теперь сохраняем промежуточные шаги вместо";
  for (const padding of ["  ", "&#x20; ", "\u00a0 "])
    assert.equal(
      cleanTerminalCopy(first + "\n" + padding + "одного финального)."),
      first + " одного финального).",
    );
  assert.equal(
    cleanTerminalCopy("  " + first + "\n  одного финального).", 200),
    "  " + first + " одного финального).",
  );
});
test("paragraphs, lists, tables and code keep their layout", async () => {
  const { cleanTerminalCopy } = await library;
  for (const text of [
    "Первый абзац.\n\nВторой абзац.",
    "- Первый пункт\n- Второй пункт",
    "  code\n    nested  x\n",
    "| name | value |\n| one  | two   |",
    "```python\ntext = 'a very long line of source code that must not be changed or merged'\n  continuation\n```",
    "⏺ Это достаточно длинное описание результата работы приложения, которое закончилось здесь.\n⏺ А это уже новое сообщение.",
  ])
    assert.equal(cleanTerminalCopy(text, 80), text);
});
test("prose removes repeated spaces and joins near-margin continuations", async () => {
  const { cleanTerminalCopy } = await library;
  const line = "Это достаточно длинное предложение для узкой панели терминала";
  for (const spaces of ["  ", "   ", "     "]) {
    assert.equal(cleanTerminalCopy("Первое предложение." + spaces + "Второе предложение."),
      "Первое предложение. Второе предложение.");
    assert.equal(cleanTerminalCopy(line + "." + spaces + "\n  Следующее предложение.", 64),
      line + ". Следующее предложение.");
  }
  assert.equal(
    cleanTerminalCopy(line + "\n  с  продолжением.", 64),
    line + " с продолжением.",
  );
  assert.equal(
    cleanTerminalCopy("Короткая строка\n  Другая строка", 80),
    "Короткая строка\n  Другая строка",
  );
});
