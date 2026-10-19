---
name: sushiai-artifacts
description: Only inside a sushiAI session (SUSHIAI_SESSION_ID is set). Show the owner a document you wrote - a plan, research, report, deck or diagram - in a sushiAI Preview half of your own pane. Works for Claude Code and Codex, in any project and on any host. Use when the owner asks to show, open or "make it a doc", a Markdown (.md) file, a presentation, slides or a deck, a diagram, a flowchart or block diagram, when the owner asks to design, draw or show an architecture, the order of steps of an agent or a skill, or a change as-is / to-be (write an editable `.arch.json` canvas), and after you write a plan or report the owner should read. Not for scratch notes.
---

# sushiAI artifacts

## First check

Run `echo "${SUSHIAI_SESSION_ID:-none}"`. If it prints `none`, you are not
inside sushiAI and this skill does not apply: stop reading it and do the task
the usual way (plain Markdown, Mermaid, HTML). Do not write `.arch.json` and
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
  approve one. Write an `.arch.json` canvas, not Mermaid: the owner edits it
  in the Preview and you both work on the same file.

Do not open scratch notes, logs or code files.

## Where to write

Write into `artifacts/<slug>.<ext>` in the project (create the folder; make
sure it is git-ignored) unless the owner names another place. One topic is
one file. Edit the same file on every revision - the Preview updates by itself.

## Formats

| Want                                         | Write                                                                 |
| -------------------------------------------- | --------------------------------------------------------------------- |
| Text, research                               | `.md` (GitHub Markdown: headings, tables, task lists, code blocks)    |
| Plan                                         | `.md` with `kind: plan` in the frontmatter (see below)                |
| Deck                                         | one `.html` with reveal.js from a CDN (template below)                |
| Sequence diagram, chart                      | one `.html` with Mermaid from a CDN, or an `.svg` file                |
| Diagram (architecture, steps, as-is / to-be) | `docs/arch/<slug>.arch.json` (opens as an editable canvas, see below) |
| Image, PDF                                   | the file as it is (`.png`, `.jpg`, `.svg`, `.pdf`)                    |

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

## Diagrams (.arch.json)

Use a diagram for an architecture, an order of steps of an agent or a skill,
or a change as-is / to-be. Write one file per view at
`docs/arch/<slug>.arch.json`. The Preview opens it as an editable canvas.

One block type covers all three. Optional properties set its look. The file
is JSON; `version` is always 1.

| Property  | Meaning                                                                                                                                                                                                                        |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `shape`   | `box` step or component; `pill` start, end or action; `diamond` decision; `cylinder` data store; `person` role; `circle` start or end dot                                                                                      |
| `icon`    | One of: `laptop` `server` `database` `cloud` `phone` `globe` `bot` `user` `users` `terminal` `code` `puzzle` `file` `folder` `message` `key` `lock` `zap` `play` `flag` `check` `git` `cpu` `box`. Another name shows no icon. |
| `caption` | Small uppercase line above the title, such as `Step` or `Skill`                                                                                                                                                                |
| `tech`    | One line under the title: technology, transport or role                                                                                                                                                                        |
| `step`    | Order number badge: a whole number from 1 to 999                                                                                                                                                                               |
| `delta`   | `new`, `changed`, `exists` or `removed`. Absent means no mark.                                                                                                                                                                 |

Sizes: box 170 by 62, pill 170 by 44, diamond 150 by 90, cylinder 170 by 72,
person 120 by 84, circle 72 by 72.

- `new` is to-be only. `changed` exists and will change. `exists` is as-is.
  `removed` exists and will go. The Preview marks each block, and the title
  shows how many are new, changed or removed.
- A group is a boundary (a trust zone, a network) or a lane (one actor per
  row). A block joins a group with its `group` id.
- An edge `label` is the text on the line. `style` is `plain` (default),
  `dashed`, or `guess` (not confirmed). `arrow` is `end` (default), `both`
  or `none`.
- A note is a question for the owner. One question per note.
- `"status": "proposed"` on a block or an edge: the owner accepts it (the
  field is removed) or rejects it (the item is deleted).
- Ids are stable slugs. Never rename an id.
- `x` and `y` are optional. Leave them out on new blocks: the Preview places
  them. Use even numbers (2 px grid).

Example a, app architecture (3 blocks, 1 group):

```json
{
  "version": 1,
  "groups": [{ "id": "cloud", "label": "Cloud", "icon": "cloud" }],
  "nodes": [
    { "id": "web", "title": "Web app", "tech": "React" },
    { "id": "api", "title": "API", "tech": "HTTPS", "group": "cloud" },
    { "id": "db", "title": "Database", "shape": "cylinder", "group": "cloud" }
  ],
  "edges": [
    { "id": "web-api", "from": "web", "to": "api", "label": "HTTPS" },
    { "id": "api-db", "from": "api", "to": "db" }
  ],
  "notes": []
}
```

Example b, order of steps (start, 2 steps, a decision):

```json
{
  "version": 1,
  "groups": [],
  "nodes": [
    { "id": "start", "title": "Start", "shape": "circle" },
    { "id": "read", "title": "Read", "step": 1 },
    { "id": "plan", "title": "Plan", "step": 2 },
    { "id": "ok", "title": "Approved?", "shape": "diamond" },
    { "id": "end", "title": "Done", "shape": "circle" }
  ],
  "edges": [
    { "id": "s-r", "from": "start", "to": "read" },
    { "id": "r-p", "from": "read", "to": "plan" },
    { "id": "p-ok", "from": "plan", "to": "ok" },
    { "id": "ok-end", "from": "ok", "to": "end", "label": "yes" },
    {
      "id": "ok-p",
      "from": "ok",
      "to": "plan",
      "label": "no",
      "style": "dashed"
    }
  ],
  "notes": []
}
```

Example c, module as-is and to-be (one exists, one changed, one new, one removed):

```json
{
  "version": 1,
  "groups": [],
  "nodes": [
    { "id": "login", "title": "Login", "delta": "exists" },
    { "id": "token", "title": "Token check", "delta": "changed" },
    { "id": "mfa", "title": "Second factor", "delta": "new" },
    { "id": "basic", "title": "Basic auth", "delta": "removed" }
  ],
  "edges": [
    { "id": "login-token", "from": "login", "to": "token" },
    { "id": "token-mfa", "from": "token", "to": "mfa" }
  ],
  "notes": []
}
```

Editing:

- The owner also edits this file in the Preview. Their edits change the file.
- Read the file again right before every edit.
- Change only the items you mean to change. Never move what the owner placed.
- Keep fields you do not know. Do not reorder items.
- Check that the file is valid JSON before you save it.
- Open it with the command in `## Open it`, with the path of the `.arch.json` file.
