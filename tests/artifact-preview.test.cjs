const { test } = require("node:test");
const assert = require("node:assert/strict");

const artifact = import("../src/extensions/preview/artifact.ts");
const worktree = import("../src/workspace/worktree.ts");

test("kindOf maps extensions and defaults to text", async () => {
  const { kindOf } = await artifact;
  assert.equal(kindOf("/a/Plan.MD"), "markdown");
  assert.equal(kindOf("/a/deck.html"), "html");
  assert.equal(kindOf("/a/x.png"), "image");
  assert.equal(kindOf("/a/x.pdf"), "pdf");
  assert.equal(kindOf("/a/x.svg"), "svg");
  assert.equal(kindOf("/a.d/notes"), "text");
  assert.equal(kindOf("/a/x.log"), "text");
});

test("parseFrontmatter reads leading key: value lines", async () => {
  const { parseFrontmatter } = await artifact;
  const doc =
    '---\nkind: plan\ntitle: "Result view"\nverified: yes\n---\n\n# Hi\n';
  assert.deepEqual(parseFrontmatter(doc), {
    meta: { kind: "plan", title: "Result view", verified: "yes" },
    body: "# Hi\n",
  });
  assert.deepEqual(parseFrontmatter("# No front\n"), {
    meta: {},
    body: "# No front\n",
  });
  assert.equal(
    parseFrontmatter("---\nkind: plan\n# unclosed").body.length > 0,
    true,
  );
  assert.deepEqual(parseFrontmatter("---\nkind: plan\n# unclosed").meta, {});
});

test("isVerified is true only for an explicit yes", async () => {
  const { isVerified, parseFrontmatter } = await artifact;
  assert.equal(
    isVerified(parseFrontmatter("---\nverified: yes\n---\nx").meta),
    true,
  );
  assert.equal(
    isVerified(parseFrontmatter("---\nverified: no\n---\nx").meta),
    false,
  );
  assert.equal(
    isVerified(parseFrontmatter("---\nkind: plan\n---\nx").meta),
    false,
  );
  assert.equal(isVerified({}), false);
});

test("planSections finds the title, goal and done-when items", async () => {
  const { planSections } = await artifact;
  const body = [
    "# Result view",
    "",
    "## Context",
    "Old.",
    "",
    "## Goal",
    "A doc opens on the right.",
    "",
    "## Done when",
    "- Opens in 1 s",
    "- [ ] 3 comments make one message",
    "1. Numbered item",
    "",
    "## Verify",
    "- npm test",
  ].join("\n");
  assert.deepEqual(planSections(body), {
    title: "Result view",
    goal: "A doc opens on the right.",
    doneWhen: ["Opens in 1 s", "3 comments make one message", "Numbered item"],
  });
  assert.deepEqual(planSections("just text"), {
    title: "",
    goal: "",
    doneWhen: [],
  });
});

test("goalPrompt sends the body, or a pointer past 12000 chars", async () => {
  const { goalPrompt } = await artifact;
  assert.equal(goalPrompt("# P\n\nDo it.\n", "/r/p.md"), "/goal # P\n\nDo it.");
  const exact = "x".repeat(12000);
  assert.equal(goalPrompt(exact, "/r/p.md"), "/goal " + exact);
  const long = goalPrompt("x".repeat(12001), "/r/my plan.md");
  assert.equal(
    long,
    "/goal Carry out the plan in /r/my plan.md. Read it first: it has the goal, context, steps, done-when, verify and stop rules.",
  );
  assert.ok(long.length < 16384);
});

test("commentMessage numbers comments and quotes the selected text", async () => {
  const { commentMessage } = await artifact;
  assert.equal(
    commentMessage("/r/p.md", [
      { quote: "All comments go\nback as one", note: "Check this" },
      { note: "General note\nsecond line" },
      { where: "slide #/2", quote: "Node A", note: "Rename" },
      { where: "point at 42%, 18%", note: "Too dark" },
    ]),
    "[sushiAI Preview] Owner feedback, 4 comments on /r/p.md\n\n1. > All comments go back as one\n   Check this\n2. General note\n   second line\n3. (slide #/2) > Node A\n   Rename\n4. (point at 42%, 18%) Too dark\n\nApply them to that file, then answer each in one line.",
  );
});

test("pasteOf brackets the message so newlines stay literal", async () => {
  const { pasteOf } = await artifact;
  assert.equal(pasteOf("a\nb"), "\x1b[200~a\nb\x1b[201~");
});

test("branchFor makes a valid feature branch", async () => {
  const { branchFor } = await artifact;
  const { worktreeBranchError } = await worktree;
  assert.equal(
    branchFor("Result view: slices 1-2!"),
    "feature/result-view-slices-1-2",
  );
  for (const title of [
    "",
    "  ..  ",
    "Ünïcode only: \u0451\u0436",
    "a".repeat(200),
    "x.lock",
    "-bad-",
    "foo/../bar",
  ]) {
    const name = branchFor(title);
    assert.equal(worktreeBranchError(name), "", name);
    assert.ok(name.startsWith("feature/"));
  }
  assert.equal(branchFor(""), "feature/plan");
});

test("recent files keep eight, newest first, without duplicates", async () => {
  const { parseRecent, pushRecent } = await artifact;
  let list = [];
  for (let i = 0; i < 10; i++) list = pushRecent(list, `/a/${i}.md`);
  list = pushRecent(list, "/a/5.md");
  assert.equal(list.length, 8);
  assert.equal(list[0], "/a/5.md");
  assert.equal(list.filter((item) => item === "/a/5.md").length, 1);
  assert.deepEqual(parseRecent("not json"), []);
  assert.deepEqual(parseRecent(JSON.stringify(["/a.md", 3])), ["/a.md"]);
});

test("acceptAnnotation accepts only the page's own well-formed message", async () => {
  const { acceptAnnotation } = await artifact;
  const message = {
    sushiai: "annotate",
    kind: "element",
    quote: "q".repeat(900),
    where: "w".repeat(300),
    rect: { x: 1, y: 2, w: 3, h: "bad" },
  };
  assert.equal(acceptAnnotation(message, false), null);
  assert.equal(acceptAnnotation({ ...message, sushiai: "mode" }, true), null);
  assert.equal(acceptAnnotation({ ...message, kind: "evil" }, true), null);
  assert.equal(acceptAnnotation("text", true), null);
  assert.equal(acceptAnnotation(null, true), null);
  const ok = acceptAnnotation(message, true);
  assert.equal(ok.quote.length, 500);
  assert.equal(ok.where.length, 120);
  assert.deepEqual(ok.rect, { x: 1, y: 2, w: 3, h: 0 });
  assert.equal(
    acceptAnnotation({ sushiai: "annotate", kind: "cancel" }, true).kind,
    "cancel",
  );
});

// eslint-disable-next-line no-control-regex
const CONTROL = /[\x00-\x08\x0b-\x1f\x7f-\x9f]/;

test("a hostile comment cannot break out of the bracketed paste", async () => {
  const { commentMessage, pasteOf } = await artifact;
  const hostile = "x\x1b[201~\x1b[Z\x03\x15y";
  const paste = pasteOf(
    commentMessage(`/p/a${hostile}.md`, [
      { quote: hostile, where: hostile, note: hostile },
      { quote: "\x1b[200~", note: "\t\x1b" },
    ]),
  );
  assert.equal(paste.split("\x1b[200~").length - 1, 1);
  assert.equal(paste.split("\x1b[201~").length - 1, 1);
  assert.ok(paste.startsWith("\x1b[200~") && paste.endsWith("\x1b[201~"));
  const inner = paste.slice(6, -6);
  assert.ok(!CONTROL.test(inner), JSON.stringify(inner));
  assert.ok(inner.includes("[201~"));
});

test("goalPrompt makes a plan safe to type into a terminal", async () => {
  const { goalPrompt } = await artifact;
  const prompt = goalPrompt("# P\r\n\tstep\x1b[201~\x03 one\x15\n", "/p/a.md");
  assert.equal(prompt, "/goal # P\n  step[201~ one");
  // eslint-disable-next-line no-control-regex
  assert.ok(!/[\x00-\x09\x0b-\x1f\x7f-\x9f]/.test(prompt));
  const long = goalPrompt("x".repeat(13000), "/p/a\x1b.md");
  assert.ok(long.includes("/p/a.md") && !long.includes("\x1b"));
});

test("insideProject accepts only absolute files under the project folder", async () => {
  const { insideProject } = await artifact;
  assert.equal(
    insideProject("/w/proj/artifacts/a.md", "/w/proj"),
    "/w/proj/artifacts/a.md",
  );
  assert.equal(insideProject("/w/proj//a.md", "/w/proj/"), "/w/proj/a.md");
  for (const bad of [
    "/etc/passwd",
    "/w/proj-other/a.md",
    "/w/proj/../secret.md",
    "/w/proj/a/../../x.md",
    "a.md",
    "/w/proj",
    "/w/proj/a\0.md",
    "",
  ])
    assert.equal(insideProject(bad, "/w/proj"), null, bad);
  assert.equal(insideProject("/w/a.md", ""), null);
  // Only the workspace folder is a root: a pane folder elsewhere widens nothing.
  assert.equal(insideProject("/w/other/artifacts/a.md", "/w/proj"), null);
  assert.equal(insideProject("/w/other/../proj/a.md", "/w/proj"), null);
  assert.equal(insideProject("rel/a.md", "/w/proj"), null);
  assert.equal(insideProject("/w/a.md", "/w/../w"), null);
});

test("the orchestrator start uses an accepted task source", async () => {
  const { startOrchestratorTask } = await artifact;
  const calls = [];
  const id = await startOrchestratorTask(
    {
      taskCreate: async (repo, params) => {
        calls.push({ repo, params });
        return { id: "t7" };
      },
    },
    "/w/proj",
    "  Do it\n",
    "/w/proj/artifacts/a.md",
  );
  assert.equal(id, "t7");
  assert.deepEqual(calls, [
    {
      repo: "/w/proj",
      params: {
        request: "Do it\n\nPlan file: /w/proj/artifacts/a.md",
        start: true,
        source: "ui",
      },
    },
  ]);
  const model = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "../orchd/src/model.rs"),
    "utf8",
  );
  const sources = /TASK_SOURCES: \[&str; \d+\] = \[([^\]]*)\]/.exec(model)[1];
  assert.match(sources, /"ui"/);
});

test("parseFrontmatter strips a leading byte order mark", async () => {
  const { parseFrontmatter } = await artifact;
  assert.deepEqual(parseFrontmatter("﻿---\nkind: plan\n---\nbody"), {
    meta: { kind: "plan" },
    body: "body",
  });
});

test("clampBoxX keeps a comment box inside the pane", async () => {
  const { clampBoxX } = await artifact;
  assert.equal(clampBoxX(50, 600), 50);
  assert.equal(clampBoxX(500, 600), 292);
  assert.equal(clampBoxX(-20, 600), 8);
  assert.equal(clampBoxX(100, 200), 8);
  assert.equal(clampBoxX(300, 400, 120), 272);
});

test("withComments keeps unsent comments per file", async () => {
  const { withComments } = await artifact;
  const a = withComments({}, "/p/a.md", [{ note: "one" }]);
  const both = withComments(a, "/p/b.md", [{ note: "two" }]);
  assert.deepEqual(both["/p/a.md"], [{ note: "one" }]);
  assert.deepEqual(both["/p/b.md"], [{ note: "two" }]);
  const cleared = withComments(both, "/p/a.md", []);
  assert.deepEqual(Object.keys(cleared), ["/p/b.md"]);
  assert.deepEqual(both["/p/a.md"], [{ note: "one" }]);
});

test("shownPath is a name in artifacts/, relative in the project, absolute outside", async () => {
  const { shownPath } = await import("../src/extensions/preview/artifact.ts");
  assert.equal(shownPath("/w/p/artifacts/plan.md", "/w/p"), "plan.md");
  assert.equal(shownPath("/w/p/docs/plan.md", "/w/p/"), "docs/plan.md");
  assert.equal(shownPath("/w/pp/plan.md", "/w/p"), "/w/pp/plan.md");
});

test("a pane folder outside the workspace cannot widen the Preview scope", async () => {
  const { insideProject, projectRelative } = await artifact;
  const { absoluteArg } = await import("../src/extensions/openSignal.ts");
  // A relative signal argument resolves from the pane folder, then must still
  // be inside the workspace folder.
  const inside = absoluteArg("artifacts/a.md", "/w/proj/sub");
  assert.equal(insideProject(inside, "/w/proj"), "/w/proj/sub/artifacts/a.md");
  const outside = absoluteArg("artifacts/a.md", "/w/proj-wt");
  assert.equal(insideProject(outside, "/w/proj"), null);
  assert.equal(insideProject(absoluteArg("a.md", "/"), "/w/proj"), null);
  assert.equal(insideProject("/etc/passwd", "/w/proj"), null);
  assert.equal(
    projectRelative("/w/proj/sub/artifacts/a.md", "/w/proj/"),
    "sub/artifacts/a.md",
  );
  assert.equal(projectRelative("/w/other/a.md", "/w/proj"), "");
});

test("a plan's agent ended when no workspace runs its branch after the grace period", async () => {
  const { agentEnded, nextBranch } = await artifact;
  const record = {
    kind: "agent",
    agent: "claude",
    branch: "feature/x",
    at: 1000,
  };
  assert.equal(agentEnded(record, ["feature/x"], 99_000), false);
  assert.equal(agentEnded(record, [], 5_000), false, "grace period");
  assert.equal(agentEnded(record, [], 99_000), true);
  assert.equal(agentEnded({ ...record, at: undefined }, [], 0), true);
  assert.equal(agentEnded(record, undefined, 99_000), false);
  assert.equal(agentEnded(undefined, [], 99_000), false);
  assert.equal(nextBranch("feature/x"), "feature/x-2");
  assert.equal(nextBranch("feature/x-2"), "feature/x-3");
});

test("editedMessage names the file and strips control characters", async () => {
  const { editedMessage } = await artifact;
  assert.equal(
    editedMessage("/p/plan.md"),
    "[sushiAI Preview] The owner edited /p/plan.md. Reread it before you change it.",
  );
  assert.ok(!editedMessage("/p/a\x1b[201~b.md").includes("\x1b"));
});

test("planSegments cuts the Goal and Done when sections out of a plan", async () => {
  const { planSegments } = await artifact;
  const body = [
    "# Title",
    "",
    "## Goal",
    "Ship it.",
    "",
    "### Why",
    "Because.",
    "",
    "## Steps",
    "1. one",
    "",
    "## Done when",
    "- a",
    "- b",
    "",
    "```",
    "## Goal",
    "```",
  ].join("\n");
  const segments = planSegments(body);
  assert.deepEqual(
    segments.map((segment) => segment.kind),
    ["text", "goal", "text", "done"],
  );
  assert.equal(segments[0].text, "# Title\n");
  assert.equal(segments[1].text, "Ship it.\n\n### Why\nBecause.\n");
  assert.ok(segments[2].text.startsWith("## Steps"));
  assert.ok(segments[3].text.startsWith("## Done when"));
  assert.ok(segments[3].text.includes("## Goal"));
  assert.deepEqual(planSegments("No sections\n"), [
    { kind: "text", text: "No sections\n" },
  ]);
});

test("an element comment carries the picked element's selector", async () => {
  const { commentMessage, acceptAnnotation } = await artifact;
  const picked = acceptAnnotation(
    {
      sushiai: "annotate",
      kind: "element",
      quote: "Preview",
      where: "How",
      selector: "g#flowchart-P-4.node",
      rect: {},
    },
    true,
  );
  assert.equal(picked.selector, "g#flowchart-P-4.node");
  const text = commentMessage("/r/d.html", [
    {
      quote: "Preview",
      where: "How",
      selector: "g#flowchart-P-4.node",
      note: "Rename",
    },
  ]);
  assert.ok(
    text.includes(
      "1. (How) > Preview\n   element: `g#flowchart-P-4.node`\n   Rename",
    ),
  );
});
