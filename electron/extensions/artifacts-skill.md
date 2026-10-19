---
name: sushiai-artifacts
description: Only inside a sushiAI session (SUSHIAI_SESSION_ID is set). Show the owner a document you wrote - a plan, research, report, deck or diagram - in a sushiAI Preview half of your own pane. Works for Claude Code and Codex, in any project and on any host. Use when the owner asks to show, open or "make it a doc", a Markdown (.md) file, a presentation, slides or a deck, a diagram, a flowchart or block diagram, when the owner asks to design, draw or show an architecture, the order of steps of an agent or a skill, or a change as-is / to-be (write an editable `.canvas` file), and after you write a plan or report the owner should read. Not for scratch notes.
---

# sushiAI artifacts

## First check

Run `echo "${SUSHIAI_SESSION_ID:-none}"`. If it prints `none`, you are not
inside sushiAI and this skill does not apply: stop reading it and do the task
the usual way (plain Markdown, Mermaid, HTML). Do not write `.canvas` files and
do not run `sushiai open` there.

sushiAI can open a file you wrote in a Preview half of your
pane. The owner reads it there, can select text and comment, and can start
a plan as a task. Works on every host and with any agent.

## When

- The owner asks to see something: a plan, research, a report, a Markdown
  file, slides or a deck, a diagram or a flowchart.
- You finished a plan or a report the owner must read before you go on.
- The owner asks to design, draw or show an architecture, a flowchart or
  block diagram, an order of steps of an agent or a skill, or a change as-is / to-be, or you want the owner to
  approve one. Write a `.canvas` file, not Mermaid: the owner edits it
  in the Preview and you both work on the same file.

Do not open scratch notes, logs or code files.

## Where to write

Write into `artifacts/<slug>.<ext>` in the project (create the folder; make
sure it is git-ignored) unless the owner names another place. One topic is
one file. Edit the same file on every revision - the Preview updates by itself.

## Formats

| Want                                         | Write                                                              |
| -------------------------------------------- | ------------------------------------------------------------------ |
| Text, research                               | `.md` (GitHub Markdown: headings, tables, task lists, code blocks) |
| Plan                                         | `.md` with `kind: plan` in the frontmatter (see below)             |
| Deck                                         | one `.html` with reveal.js from a CDN (template below)             |
| Sequence diagram, chart                      | one `.html` with Mermaid from a CDN, or an `.svg` file             |
| Diagram (architecture, steps, as-is / to-be) | `docs/arch/<slug>.canvas` (opens as an editable canvas, see below) |
| Image, PDF                                   | the file as it is (`.png`, `.jpg`, `.svg`, `.pdf`)                 |

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

After the file is written, run this one command. The path must be absolute
(`$(pwd)/artifacts/<slug>.md` works):

```sh
sushiai open artifacts/preview "$(pwd)/artifacts/<slug>.md"
```

Run the command again only to bring a closed Preview back or to switch it to
another file.

## Comments

The owner's comments arrive in your input as one message that starts with
`[sushiAI Preview] Owner feedback, N comments on <path>`, each with the
quoted passage. Apply them to the same
file, then answer in one short line per comment.

The owner can also edit a Markdown file in the Preview. Then you get one
line, `[sushiAI Preview] The owner edited <path>. Reread it before you change
it.` Read the file again before your next edit, so you never write back an
old copy.

## Diagrams (.canvas)

Use a diagram for an architecture, an order of steps of an agent or a skill,
or a change as-is / to-be. Write one file per view at
`docs/arch/<slug>.canvas`. The file is a JSON Canvas 1.0 file. The Preview
opens it as an editable canvas. The file opens in Obsidian and other JSON
Canvas tools too. They may drop the `sushiai` fields when they save.

The file is JSON with three top-level keys: `nodes`, `edges` and `sushiai`.
`sushiai.version` is always 1. `sushiai.title` is the title of the view.

### Blocks

A block is a `text` node. Its `text` is the title. A second line is the
subtitle, such as the technology, the transport or the role. Give every node
integer `x`, `y`, `width` and `height`. Use even numbers (2 px grid). Sizes
per shape:

| Shape      | Width by height | Use                  |
| ---------- | --------------- | -------------------- |
| `box`      | 170 by 62       | step or component    |
| `pill`     | 170 by 44       | start, end or action |
| `diamond`  | 150 by 90       | decision             |
| `cylinder` | 170 by 72       | data store           |
| `person`   | 120 by 84       | role                 |
| `circle`   | 72 by 72        | start or end dot     |

A block sizes itself: the Preview widens it to its text (up to 320 px), wraps
the rest and makes it taller, so a long title or subtitle is never cut off.
Your `width` and `height` are only the size other tools draw. To fix the size,
set `"size": "fixed"` in the `sushiai` object of the node; the text then wraps
in that width.

Extras go in the `sushiai` object of the node. Set only the keys you use:

| Key       | Meaning                                                                                                                                                                                                                        |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `shape`   | One of the shapes above. Absent means `box`.                                                                                                                                                                                   |
| `icon`    | One of: `laptop` `server` `database` `cloud` `phone` `globe` `bot` `user` `users` `terminal` `code` `puzzle` `file` `folder` `message` `key` `lock` `zap` `play` `flag` `check` `git` `cpu` `box`. Another name shows no icon. |
| `caption` | Small uppercase line above the title, such as `Step` or `Skill`.                                                                                                                                                               |
| `step`    | Order number badge: a whole number from 1 to 999.                                                                                                                                                                              |
| `status`  | `proposed` (see Proposals below).                                                                                                                                                                                              |
| `delta`   | `new`, `changed`, `exists` or `removed`. Absent means no mark.                                                                                                                                                                 |

Delta:

- `new` is to-be only. `changed` exists and will change. `exists` is as-is.
  `removed` exists and will go.
- Set `color` to match the delta: `new` is `"4"`, `changed` is `"3"` and
  `removed` is `"1"`. `exists` has no `color`.
- The Preview marks each block, and the title shows how many are new, changed
  or removed.

### Groups

A group is a `group` node with `label`, `x`, `y`, `width` and `height`. It is
a boundary (a trust zone, a network) or a lane (one actor per row). Optional
`sushiai.icon` takes the same names as a block icon.

A block belongs to the group whose rectangle contains the centre of the block.
Do not write a group id on a block. Place the block fully inside the group.
A group is exactly the `x`, `y`, `width` and `height` you write; the Preview
does not pad or grow it. Leave room: about 30 px above the first block for the
label, and 10 px at the sides and 8 px below.

### Notes

A note is a question for the owner. It is a `text` node with
`"sushiai": { "note": true }` and `"color": "3"`. Its `text` is the question.
One question per note. The text may have several lines; the Preview wraps
long lines, so do not set the note size.

### Edges

- `fromNode` and `toNode` are the ids of the two nodes.
- `label` is the text on the line.
- `toEnd` is `arrow` by default. `fromEnd` is `none` by default. Write them
  only when they differ. For both arrows set `fromEnd` to `arrow`. For no
  arrow set `toEnd` to `none`. For an arrow at `fromNode` only set `fromEnd`
  to `arrow` and `toEnd` to `none`.
- `sushiai.style` is `dashed` or `guess`. `guess` means not confirmed; set
  `color` to `"3"` as well. Absent means a plain line.
- `sushiai.status` is `proposed` (see Proposals below).

### Proposals

`"status": "proposed"` in `sushiai` on a node or an edge marks an item the
agent proposed. The owner accepts it (the field is removed) or rejects it (the
item is deleted).

### Ids and layout

- Every node and every edge has a unique id. Ids are stable slugs. Never rename
  an id.
- Leave at least 40 px between blocks. No two blocks overlap.
- Do not reorder items.

Example a, app architecture (one group, one cylinder):

```json
{
  "nodes": [
    {
      "id": "cloud",
      "type": "group",
      "label": "Cloud",
      "x": 40,
      "y": 40,
      "width": 420,
      "height": 240,
      "sushiai": { "icon": "cloud" }
    },
    {
      "id": "web",
      "type": "text",
      "text": "Web app\nReact",
      "x": 40,
      "y": 320,
      "width": 170,
      "height": 62,
      "sushiai": { "icon": "globe" }
    },
    {
      "id": "api",
      "type": "text",
      "text": "API\nHTTPS",
      "x": 100,
      "y": 100,
      "width": 170,
      "height": 62,
      "sushiai": { "icon": "server" }
    },
    {
      "id": "db",
      "type": "text",
      "text": "Database\nPostgreSQL",
      "x": 100,
      "y": 202,
      "width": 170,
      "height": 72,
      "sushiai": { "shape": "cylinder", "icon": "database" }
    }
  ],
  "edges": [
    {
      "id": "web-api",
      "fromNode": "web",
      "toNode": "api",
      "label": "HTTPS"
    },
    {
      "id": "api-db",
      "fromNode": "api",
      "toNode": "db"
    }
  ],
  "sushiai": { "version": 1, "title": "App architecture" }
}
```

Example b, order of steps (start, two steps, a decision):

```json
{
  "nodes": [
    {
      "id": "start",
      "type": "text",
      "text": "Start",
      "x": 40,
      "y": 100,
      "width": 72,
      "height": 72,
      "sushiai": { "shape": "circle" }
    },
    {
      "id": "read",
      "type": "text",
      "text": "Read",
      "x": 152,
      "y": 106,
      "width": 170,
      "height": 62,
      "sushiai": { "caption": "Step", "step": 1 }
    },
    {
      "id": "plan",
      "type": "text",
      "text": "Plan",
      "x": 362,
      "y": 106,
      "width": 170,
      "height": 62,
      "sushiai": { "caption": "Step", "step": 2 }
    },
    {
      "id": "ok",
      "type": "text",
      "text": "Approved?",
      "x": 574,
      "y": 92,
      "width": 150,
      "height": 90,
      "sushiai": { "shape": "diamond" }
    },
    {
      "id": "end",
      "type": "text",
      "text": "Done",
      "x": 764,
      "y": 100,
      "width": 72,
      "height": 72,
      "sushiai": { "shape": "circle" }
    }
  ],
  "edges": [
    {
      "id": "s-r",
      "fromNode": "start",
      "toNode": "read"
    },
    {
      "id": "r-p",
      "fromNode": "read",
      "toNode": "plan"
    },
    {
      "id": "p-ok",
      "fromNode": "plan",
      "toNode": "ok"
    },
    {
      "id": "ok-end",
      "fromNode": "ok",
      "toNode": "end",
      "label": "yes"
    },
    {
      "id": "ok-p",
      "fromNode": "ok",
      "toNode": "plan",
      "label": "no",
      "sushiai": { "style": "dashed" }
    }
  ],
  "sushiai": { "version": 1, "title": "Agent task steps" }
}
```

Example c, one module as-is and to-be (one exists, one changed, one new, one
removed):

```json
{
  "nodes": [
    {
      "id": "login",
      "type": "text",
      "text": "Login\nPassword form",
      "x": 40,
      "y": 100,
      "width": 170,
      "height": 62,
      "sushiai": { "delta": "exists" }
    },
    {
      "id": "token",
      "type": "text",
      "text": "Token check\nAPI",
      "x": 252,
      "y": 100,
      "width": 170,
      "height": 62,
      "color": "3",
      "sushiai": { "delta": "changed" }
    },
    {
      "id": "mfa",
      "type": "text",
      "text": "Second factor\nTOTP",
      "x": 464,
      "y": 100,
      "width": 170,
      "height": 62,
      "color": "4",
      "sushiai": { "delta": "new" }
    },
    {
      "id": "basic",
      "type": "text",
      "text": "Basic auth\nLegacy",
      "x": 252,
      "y": 220,
      "width": 170,
      "height": 62,
      "color": "1",
      "sushiai": { "delta": "removed" }
    }
  ],
  "edges": [
    {
      "id": "login-token",
      "fromNode": "login",
      "toNode": "token"
    },
    {
      "id": "token-mfa",
      "fromNode": "token",
      "toNode": "mfa"
    },
    {
      "id": "login-basic",
      "fromNode": "login",
      "toNode": "basic",
      "sushiai": { "style": "dashed" }
    }
  ],
  "sushiai": { "version": 1, "title": "Login as-is and to-be" }
}
```

Editing:

- The owner also edits this file in the Preview. Their edits change the file.
- Read the file again right before every edit.
- Change only the items you mean to change. Never move what the owner placed.
- Keep fields you do not know. Do not reorder items.
- Check that the file is valid JSON before you save it.
- Open it with the command in `## Open it`, with the path of the `.canvas`
  file.
