const { test } = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/app/sessionPrompt.ts");

// Visible text of real Claude Code 2.1 and Codex 0.159 panes, read with Herdr
// `pane.read --source visible --format text` at 110 columns. Only the scratch
// project path was replaced with /home/user/app, and Claude Code screens start
// at the echoed prompt instead of the conversation above it.
const SCREENS = {
  claudeEdit:
    "❯ Edit notes.txt so it says goodbye instead of hello. Use the Edit tool, nothing else.\n\n  Read 1 file\n\n⏺ Update(notes.txt)\n\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n Edit file\n notes.txt\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n 1 -hello\n 1 +goodbye\n╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌\n Do you want to make this edit to notes.txt?\n ❯ 1. Yes\n   2. Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session\n      (shift+tab)\n   3. No\n\n Esc to cancel · Tab to amend",
  claudeBash:
    "❯ Run the shell command: mkdir -p build && touch build/out.txt\n\n  Creating build directory and empty out.txt file\n  ⎿  $ mkdir -p build && touch build/out.txt\n\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n Bash command\n\n   mkdir -p build && touch build/out.txt\n   Create build directory and empty out.txt file\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for mkdir -p build and touch build/out.txt commands in /home/user/app\n   3. No\n\n Esc to cancel · Tab to amend",
  claudeAsk:
    "❯ Use the AskUserQuestion tool to ask me which accent color the app should use, with options Red and Blue.\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n ☐ Accent Color\n\nWhich accent color should the app use?\n\n❯ 1. Red\n     Bold and energetic accent color\n  2. Blue\n     Calm and professional accent color\n  3. Type something.\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n  4. Chat about this\n\nEnter to select · ↑/↓ to navigate · Esc to cancel",
  claudeMulti:
    "❯ Use the AskUserQuestion tool with multiSelect true to ask which platforms to ship: macOS, Linux, Windows.\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n←  ☐ Platforms  ✔ Submit  →\n\nWhich platforms should this ship to?\n\n❯ 1. [ ] macOS\n         Apple's operating system for Mac computers\n  2. [ ] Linux\n         Open-source operating system, typically for servers and development\n  3. [ ] Windows\n         Microsoft's operating system for PCs\n  4. [ ] Type something\n     Submit\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n  5. Chat about this\n\nEnter to select · ↑/↓ to navigate · Esc to cancel",
  claudeMultiChecked:
    "❯ Use the AskUserQuestion tool with multiSelect true to ask which platforms to ship: macOS, Linux, Windows.\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n←  ☒ Platforms  ✔ Submit  →\n\nWhich platforms should this ship to?\n\n❯ 1. [✔] macOS\n         Apple's operating system for Mac computers\n  2. [ ] Linux\n         Open-source operating system, typically for servers and development\n  3. [✔] Windows\n         Microsoft's operating system for PCs\n  4. [ ] Type something\n     Submit\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n  5. Chat about this\n\nEnter to select · ↑/↓ to navigate · Esc to cancel",
  claudeMultiReview:
    "❯ Use the AskUserQuestion tool with multiSelect true to ask which platforms to ship: macOS, Linux, Windows.\n\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n←  ☒ Platforms  ✔ Submit  →\n\nReview your answers\n\n ● Which platforms should this ship to?\n   → macOS, Windows\n\nReady to submit your answers?\n\n❯ 1. Submit answers\n  2. Cancel",
  claudeTrust:
    "\n──────────────────────────────────────────────────────────────────────────────────────────────────────────────\n Accessing workspace:\n\n /home/user/app\n\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open\n source project, or work from your team). If not, take a moment to review what's in this folder first.\n\n Claude Code'll be able to read, edit, and execute files here.\n\n Security guide\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel",
  codexTrust:
    "\n  Folder access\n  /home/user/app\n\n  Trust this folder? Codex can read, edit, and run files here, subject to your permission settings. Folder\n  settings can run code automatically, even without a model request. Continue only if you trust these files.\n  Your trust decision will be saved.\n\n› 1. Trust and continue\n  2. Back to Agent Command Center\n\n  enter continue · esc back",
  codexHooks:
    "\n  Hooks need review\n  1 hook is new or changed.\n  Hooks can run outside the sandbox after you trust them.\n\n\n› 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)\n\n  enter confirm · esc skip",
};

const labels = (prompt) => prompt.options.map((option) => option.label);
const steps = (prompt) => prompt.options.map((option) => option.steps);

test("a Claude Code edit permission is its question, the diff and digit answers", async () => {
  const { parseSessionPrompt } = await load();
  const prompt = parseSessionPrompt(SCREENS.claudeEdit, "claude");
  assert.equal(prompt.question, "Do you want to make this edit to notes.txt?");
  assert.deepEqual(prompt.detail, [
    "Edit file",
    "notes.txt",
    "1 -hello",
    "1 +goodbye",
  ]);
  assert.deepEqual(labels(prompt), [
    "Yes",
    "Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session",
    "No",
  ]);
  assert.equal(prompt.options[1].hint, "(shift+tab)");
  assert.deepEqual(steps(prompt), [["1"], ["2"], ["3"]]);
  assert.equal(prompt.multi, false);
  assert.equal(
    prompt.typeSteps,
    null,
    "typing into a permission menu would pick an option",
  );
});

test("a Claude Code Bash permission shows the command it asks about", async () => {
  const { parseSessionPrompt } = await load();
  const prompt = parseSessionPrompt(SCREENS.claudeBash, "claude");
  assert.equal(prompt.question, "Do you want to proceed?");
  assert.deepEqual(prompt.detail, [
    "Bash command",
    "mkdir -p build && touch build/out.txt",
    "Create build directory and empty out.txt file",
  ]);
  assert.deepEqual(labels(prompt), [
    "Yes",
    "Yes, and don't ask again for mkdir -p build and touch build/out.txt commands in /home/user/app",
    "No",
  ]);
});

test("a Claude Code question menu keeps its options and answers free text through Type something", async () => {
  const { parseSessionPrompt, replySteps } = await load();
  const prompt = parseSessionPrompt(SCREENS.claudeAsk, "claude");
  assert.equal(prompt.question, "Which accent color should the app use?");
  assert.deepEqual(labels(prompt), ["Red", "Blue", "Chat about this"]);
  assert.equal(prompt.options[0].hint, "Bold and energetic accent color");
  assert.deepEqual(steps(prompt), [["1"], ["2"], ["4"]]);
  assert.deepEqual(prompt.typeSteps, ["3"]);
  assert.deepEqual(replySteps(prompt, "Teal,\nplease "), [
    "3",
    "Teal, please\r",
  ]);
  assert.equal(replySteps(prompt, "   "), null);
});

test("a Claude Code multi-select toggles boxes and advances to its review", async () => {
  const { parseSessionPrompt } = await load();
  const open = parseSessionPrompt(SCREENS.claudeMulti, "claude");
  assert.equal(open.question, "Which platforms should this ship to?");
  assert.equal(open.multi, true);
  assert.deepEqual(labels(open), [
    "macOS",
    "Linux",
    "Windows",
    "Chat about this",
  ]);
  assert.deepEqual(
    open.options.map((option) => option.checked),
    [false, false, false, undefined],
  );
  assert.deepEqual(open.advance, ["\x1b[C"]);
  assert.equal(open.typeSteps, null);
  const ticked = parseSessionPrompt(SCREENS.claudeMultiChecked, "claude");
  assert.deepEqual(
    ticked.options.map((option) => option.checked),
    [true, false, true, undefined],
  );
  const review = parseSessionPrompt(SCREENS.claudeMultiReview, "claude");
  assert.equal(review.question, "Ready to submit your answers?");
  assert.deepEqual(labels(review), ["Submit answers", "Cancel"]);
  assert.deepEqual(steps(review), [["1"], ["2"]]);
  assert.equal(review.multi, false);
});

test("the unnumbered folder-trust menu is answered with arrows from the highlight", async () => {
  const { parseSessionPrompt } = await load();
  const prompt = parseSessionPrompt(SCREENS.claudeTrust, "claude");
  assert.match(prompt.question, /^Quick safety check: Is this a project/);
  assert.deepEqual(prompt.detail, ["Accessing workspace:", "/home/user/app"]);
  assert.deepEqual(labels(prompt), ["No, exit", "Yes, I trust this folder"]);
  assert.deepEqual(steps(prompt), [["\r"], ["\x1b[B\r"]]);
});

test("Codex menus are driven with arrows and Enter, never a bare digit", async () => {
  const { parseSessionPrompt } = await load();
  const trust = parseSessionPrompt(SCREENS.codexTrust, "codex");
  assert.match(trust.question, /^Trust this folder\? Codex can read/);
  assert.deepEqual(labels(trust), [
    "Trust and continue",
    "Back to Agent Command Center",
  ]);
  assert.deepEqual(steps(trust), [["\r"], ["\x1b[B\r"]]);
  const hooks = parseSessionPrompt(SCREENS.codexHooks, "codex");
  assert.equal(
    hooks.question,
    "Hooks need review 1 hook is new or changed. Hooks can run outside the sandbox after you trust them.",
  );
  assert.deepEqual(steps(hooks)[2], ["\x1b[B\x1b[B\r"]);
});

test("an unknown screen is its last meaningful line with a plain text reply", async () => {
  const { parseSessionPrompt, replySteps } = await load();
  // The Bash screen after answering No: Claude Code waits at its input box.
  const screen = [
    "  Ran 1 shell command",
    "  ⎿  Interrupted · What should Claude do instead?",
    "",
    "✻ Sautéed for 2s · done 9:48 PM",
    "",
    "─".repeat(110),
    "❯",
    "─".repeat(110),
    "  Haiku 4.5 │ app ⎇ main ███░░░░░░░ 30%",
    "  ⏸ manual mode on · ← 1 agent",
  ].join("\n");
  const prompt = parseSessionPrompt(screen, "claude");
  assert.equal(prompt.question, "Interrupted · What should Claude do instead?");
  assert.deepEqual(prompt.options, []);
  assert.deepEqual(replySteps(prompt, "try again"), ["try again\r"]);
});

test("a numbered list in the conversation is not a menu", async () => {
  const { parseSessionPrompt } = await load();
  const prompt = parseSessionPrompt(
    ["Steps:", "  1. Build", "  2. Test", "", "Done."].join("\n"),
    "claude",
  );
  assert.deepEqual(prompt.options, []);
  assert.equal(prompt.question, "Done.");
});
