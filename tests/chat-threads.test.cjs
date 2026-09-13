const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/chat-threads.ts");

test("startUserTurn parks an empty answer and names an untitled thread", async () => {
  const { startUserTurn } = await library;
  const panel = { id: "p", kind: "chat", title: "New thread", messages: [] };
  const { messages, update } = startUserTurn(
    panel,
    "Fix the build\nand tests",
    [],
    1000,
  );
  assert.equal(messages.length, 1, "the request carries only the real turns");
  assert.equal(messages[0].role, "user");
  assert.equal(update.messages.length, 2);
  assert.deepEqual(update.messages[1], {
    id: update.messages[1].id,
    role: "assistant",
    text: "",
  });
  assert.equal(update.title, "Fix the build", "titled from the first line");
  assert.equal(update.busy, true);
  assert.equal(update.updatedAt, 1000);
  assert.equal(update.attachments, undefined);

  const named = startUserTurn(
    { ...panel, title: "Release prep" },
    "hi",
    [],
    1000,
  );
  assert.equal(
    named.update.title,
    "Release prep",
    "a named thread keeps its name",
  );

  const withFiles = startUserTurn(panel, "look", ["/tmp/a.txt"], 1000);
  assert.deepEqual(withFiles.messages[0].attachments, ["/tmp/a.txt"]);
});

test("applyChatEvent appends streamed text and clears busy on done", async () => {
  const { applyChatEvent } = await library;
  const start = {
    id: "p",
    kind: "chat",
    title: "t",
    busy: true,
    messages: [
      { id: "1", role: "user", text: "hi" },
      { id: "2", role: "assistant", text: "" },
    ],
  };
  const streaming = applyChatEvent(start, { panelId: "p", text: "Hel" }, 2000);
  assert.equal(streaming.messages.at(-1).text, "Hel");
  assert.equal(streaming.busy, true);
  assert.equal(streaming.updatedAt, undefined, "only done stamps a time");

  const more = applyChatEvent(streaming, { panelId: "p", text: "lo" }, 2000);
  assert.equal(more.messages.at(-1).text, "Hello");

  const done = applyChatEvent(
    more,
    { panelId: "p", done: true, model: "opus" },
    3000,
  );
  assert.equal(done.busy, false);
  assert.equal(done.updatedAt, 3000);
  assert.equal(
    done.messages.at(-1).model,
    "opus",
    "the answer records its model",
  );
  assert.equal(start.messages.length, 2, "the input panel is untouched");
});

test("applyChatEvent drops an answer that ended empty", async () => {
  const { applyChatEvent } = await library;
  const panel = {
    id: "p",
    kind: "chat",
    title: "t",
    busy: true,
    messages: [
      { id: "1", role: "user", text: "hi" },
      { id: "2", role: "assistant", text: "" },
    ],
  };
  const done = applyChatEvent(panel, { panelId: "p", done: true }, 3000);
  assert.equal(
    done.messages.length,
    1,
    "no empty assistant bubble is left behind",
  );
  assert.equal(done.messages.at(-1).role, "user");
  assert.equal(done.busy, false);
});

test("applyChatEvent keeps an error and clears the note when the turn ends", async () => {
  const { applyChatEvent } = await library;
  const panel = {
    id: "p",
    kind: "chat",
    title: "t",
    note: "thinking",
    messages: [
      { id: "1", role: "user", text: "hi" },
      { id: "2", role: "assistant", text: "partial" },
    ],
  };
  const noted = applyChatEvent(panel, { panelId: "p", note: "running a tool" });
  assert.equal(noted.note, "running a tool");

  const failed = applyChatEvent(
    panel,
    { panelId: "p", done: true, error: "boom" },
    4000,
  );
  assert.equal(failed.error, "boom");
  assert.equal(failed.note, undefined, "a finished turn shows no live note");
  assert.equal(failed.busy, false);

  const kept = applyChatEvent(
    { ...panel, error: "earlier" },
    { panelId: "p", text: "x" },
  );
  assert.equal(
    kept.error,
    "earlier",
    "an event without an error keeps the old one",
  );
});

test("applyChatEvent stamps the resolved model when the event omits one", async () => {
  const { applyChatEvent } = await library;
  const panel = {
    id: "p",
    kind: "chat",
    title: "t",
    resolvedModel: "sonnet",
    messages: [
      { id: "1", role: "user", text: "hi" },
      { id: "2", role: "assistant", text: "answer" },
    ],
  };
  const done = applyChatEvent(panel, { panelId: "p", done: true }, 5000);
  assert.equal(done.messages.at(-1).model, "sonnet");
  assert.equal(done.resolvedModel, "sonnet");
});
