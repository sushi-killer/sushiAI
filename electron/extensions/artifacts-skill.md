---
name: sushiai-artifacts
description: Show the owner a document you wrote - a plan, research, report, deck or diagram - in a sushiAI Preview half of your own pane. Works for Claude Code and Codex, in any project and on any host. Use when the owner asks to show, open or "make it a doc", a presentation, a deck or a diagram, and after you write a plan or report the owner should read. Not for scratch notes.
---

# sushiAI artifacts

sushiAI can open a file you wrote in a Preview half of your
pane. The owner reads it there, can select text and comment, and can start
a plan as a task. Works on every host and with any agent.

## When

- The owner asks to see something: a plan, research, a report, a deck, a
  diagram.
- You finished a plan or a report the owner must read before you go on.

Do not open scratch notes, logs or code files.

## Where to write

Write into `artifacts/<slug>.<ext>` in the project (create the folder; make
sure it is git-ignored) unless the owner names another place. One topic is
one file. Edit the same file on every revision - the Preview updates by itself.

## Formats

| Want           | Write                                                              |
| -------------- | ------------------------------------------------------------------ |
| Text, research | `.md` (GitHub Markdown: headings, tables, task lists, code blocks) |
| Plan           | `.md` with `kind: plan` in the frontmatter (see below)             |
| Deck           | one `.html` with reveal.js from a CDN (template below)             |
| Diagram        | one `.html` with Mermaid from a CDN, or an `.svg` file             |
| Image, PDF     | the file as it is (`.png`, `.jpg`, `.svg`, `.pdf`)                 |

Markdown frontmatter (optional, first lines of the file):

```markdown
---
kind: research # or: plan, report
title: Short title
verified: yes # plans only, see below
---
```

A **plan** is written as a goal, because its Start task button opens a new
agent pane (Claude Code or Codex, in a new worktree) and sends the plan to it
as `/goal <plan>`. The orchestrator can run it instead. So the plan must stand
alone, with no "as discussed above". Use these sections:

- `## Goal`: one paragraph. What is true when the work is done.
- `## Context`: the files, decisions and constraints the new agent needs.
- `## Steps`: numbered, one change per step.
- `## Done when`: checks a person can verify from outside.
- `## Verify`: the commands that prove it, such as tests or a build.
- `## Stop rules`: when to stop and ask instead of guessing.

Keep a plan under about 12 000 characters. A longer plan is sent as a pointer
to the file.

### Verify a plan before you open it

A broken plan becomes a broken goal. Check the plan before you open it, and
again after each revision:

1. Facts: every file, function, command and path the plan names exists. Read
   the code; do not trust your memory or the chat.
2. No contradictions: Steps, Done when and Stop rules do not conflict, and no
   check asks for two opposite things.
3. Coverage: each Done when check is reached by at least one step, and each
   step serves a check.
4. Testable: a person can verify each Done when from outside. Each Verify
   command runs in this repo.
5. Stands alone: no "as discussed", no context that exists only in this chat.

If you can start a subagent, give it the plan and the repo with no other
context and ask it to find contradictions and gaps. Fix what it finds.
When the plan passes, set `verified: yes` in the frontmatter. If a check
fails and you cannot fix it, set `verified: no` and add a `## Open questions`
section. The Start task window shows this state to the owner.

Deck template (keep it one file, slides are `<section>`):

```html
<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <link
      rel="stylesheet"
      href="https://cdn.jsdelivr.net/npm/reveal.js@5/dist/reveal.css"
    />
    <link
      rel="stylesheet"
      href="https://cdn.jsdelivr.net/npm/reveal.js@5/dist/theme/black.css"
    />
  </head>
  <body>
    <div class="reveal">
      <div class="slides">
        <section>
          <h2>Title</h2>
          <p>One idea per slide.</p>
        </section>
      </div>
    </div>
    <script src="https://cdn.jsdelivr.net/npm/reveal.js@5/dist/reveal.js"></script>
    <script>
      Reveal.initialize({ hash: true });
    </script>
  </body>
</html>
```

Diagram template:

```html
<!doctype html>
<html>
  <body style="background:#111;color:#eee;font-family:sans-serif">
    <pre class="mermaid">
flowchart LR
  A[Agent] -->|writes| B[artifacts/plan.md]
  B --> C[Preview]
    </pre>
    <script type="module">
      import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
      mermaid.initialize({ startOnLoad: true, theme: "dark" });
    </script>
  </body>
</html>
```

## Open it

After the file is written, run this one command. The path is relative to
your current folder, or absolute. Herdr cuts each token at 80 characters,
so keep the path short and keep both tokens in one call:

```sh
herdr workspace report-metadata "$HERDR_WORKSPACE_ID" --source sushiai --token "sushiai_open=$HERDR_PANE_ID builtin.artifacts/preview $(date +%s)-$$" --token "sushiai_open_arg=artifacts/<slug>.md"
```

If `HERDR_PANE_ID` is not set you are not inside sushiAI: give the owner the
path instead. Run the command again only to bring a closed Preview back or
to switch it to another file.

## Comments

The owner's comments arrive in your input as one message that starts with
`[sushiAI Preview] Owner feedback, N comments on <path>`, each with the
quoted passage. Apply them to the same
file, then answer in one short line per comment.

The owner can also edit a Markdown file in the Preview. Then you get one
line, `[sushiAI Preview] The owner edited <path>. Reread it before you change
it.` Read the file again before your next edit, so you never write back an
old copy.
