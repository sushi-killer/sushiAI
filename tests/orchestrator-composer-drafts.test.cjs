// The orchestrator's composers keep unsent text per host, repo and composer,
// so switching to another project and back brings the draft back.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const {
  readComposerDraft,
  writeComposerDraft,
} = require("../src/orchestrator/composerDraft.ts");

function memoryStorage() {
  const map = new Map();
  return {
    map,
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
  };
}

test("a draft survives a switch to another project and back", () => {
  const storage = memoryStorage();
  writeComposerDraft("local", "/work/app", "task", "Fix the login", storage);
  assert.equal(readComposerDraft("local", "/work/site", "task", storage), "");
  writeComposerDraft("local", "/work/site", "task", "Bump deps", storage);
  assert.equal(
    readComposerDraft("local", "/work/app", "task", storage),
    "Fix the login",
  );
  assert.equal(
    readComposerDraft("local", "/work/site", "task", storage),
    "Bump deps",
  );
});

test("drafts are kept apart per host and per composer", () => {
  const storage = memoryStorage();
  writeComposerDraft("local", "/srv/app", "task", "Local task", storage);
  assert.equal(readComposerDraft("ssh:lab", "/srv/app", "task", storage), "");
  assert.equal(readComposerDraft("local", "/srv/app", "chat", storage), "");
  writeComposerDraft("local", "/srv/app", "chat", "A question", storage);
  assert.equal(
    readComposerDraft("local", "/srv/app", "task", storage),
    "Local task",
  );
});

test("an empty draft removes its entry", () => {
  const storage = memoryStorage();
  writeComposerDraft("local", "/srv/app", "task", "Something", storage);
  writeComposerDraft("local", "/srv/app", "task", "", storage);
  assert.equal(storage.map.size, 0);
  assert.equal(readComposerDraft("local", "/srv/app", "task", storage), "");
});

test("blocked storage reads an empty draft and never throws", () => {
  const blocked = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    },
    removeItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(readComposerDraft("local", "/srv/app", "task", blocked), "");
  assert.doesNotThrow(() =>
    writeComposerDraft("local", "/srv/app", "task", "Text", blocked),
  );
});

test("the task composer and the orchestrator chat keep their text in the stored drafts", () => {
  for (const file of ["OrchestratorPanel.tsx", "ChatView.tsx"]) {
    const source = fs.readFileSync(
      path.join(__dirname, "../src/orchestrator", file),
      "utf8",
    );
    assert.match(source, /readComposerDraft\(/, file);
    assert.match(source, /writeComposerDraft\(/, file);
  }
});
