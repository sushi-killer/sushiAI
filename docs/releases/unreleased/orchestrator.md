## Orchestrator: tasks carried to a verified done

- A new **Orchestrator** panel turns a request into a task that agents carry to done without you watching. A planner drafts the goal, acceptance criteria and the commands that prove them, and asks only what it cannot settle. Each task runs on a `task/<name>` branch in its own worktree under `<repo>/.sushiai/worktrees/`.
- "Done" is decided by the app, not the agent: the checks must pass and an independent read-only review must agree. The reviewer is the cheapest route at least as strong as the one that did the work, preferably on the other harness.
- A failed attempt retries in a fresh session with the failure in its brief; repeated failures move the task to a stronger model, then ask you one question with options. Routine questions are answered for you and recorded as assumptions you can overturn; budget and protected-path questions always come to you.
- Large requests become subtasks that land on one branch, and tasks on the same base never edit the same file at once.
- Tasks run in the orchd background service. It starts the first time something needs it (opening the Orchestrator panel, creating a task, answering from the mascot), keeps running while the window is closed to the menu bar, and stops when you quit sushiAI. A daemon that is already running is picked up at launch. Stopped tasks stay resumable and pick up with a fresh attempt next time.
- The orchestrator can be turned off in Extensions. While it is off, sushiAI does not start or contact orchd or its SSH hosts, and the Inbox, the menu bar badge, Settings and the panel picker look and behave as they did before the orchestrator existed. The choice is remembered, and turning it back on needs no restart.
- Agents run sandboxed (writes stay inside the task's worktree) with only the project's own MCP servers, plus tools to message sibling tasks and ask the orchestrator.

## Landing finished work

- A finished top-level task lands on its base branch by itself (**Land finished work**, on by default): its work is carried onto the branch's head, squashed to one commit, checked again and the branch moved. Nothing is ever pushed.
- A checkout with uncommitted changes is never touched; the task waits in `landing` and retries. Conflicts or failing checks go back to an agent.
- Landing on a repo's default branch is refused unless that repo is listed under Settings > Orchestration > **Repos where landing on the default branch is allowed**; otherwise the task stays done on its own branch. **Land** on a done task lands it now.

## Chat, Brainstorm and Plan

- The panel opens on **Home**: what needs you, what runs, what landed today. ⌘N focuses the composer.
- The chat has a **Chat / Brainstorm / Plan** switch and several sessions per project, and keeps answering in the background. Brainstorm asks one question at a time with answer chips; Plan splits a goal into small ordered tasks shown in a **Proposed tasks** card. The orchestrator only reads your project; it never edits files.
- Plan keeps a **Next / Later** backlog; **Autopilot** (off by default) starts ready Next tasks when a slot is free.
- **Connected tools**: add servers from `~/.claude.json` or installed Claude Code plugins in Settings > Orchestration (the list starts empty). The chat may use their read tools; anything that writes waits on an OK card and runs only when you press Send.

## Task detail, spend and improvements

- A task's detail shows its report (outcome, criteria, assumptions, cost by stage and model, follow-ups), a clickable stage timeline, diff stats and its tier. Tasks can be archived and restored, and **Needed a fix** with a note creates a follow-up task.
- **Analytics** pages through weeks of spend by stage and model and lists recurring failures. Optional budgets per task, per attempt and per day make a task wait for you instead of running.
- **Improvements** lists proposals drawn from past tasks and your **Repo notes**, which the planner reads first. Nothing is applied without your approval.

## Remote SSH hosts

- Pick Local or any Connections profile at the top of the panel. The app installs orchd into `~/.sushiai/bin` on the host and drives it over the existing SSH connection, with no new open ports. The host can be any macOS or Linux machine: the app uploads its own build when the OS and CPU match, otherwise builds orchd there from the source it ships with, using cargo; when the host has no Rust, one button in the setup checklist installs it there (a minimal toolchain in ~/.cargo, no sudo) and carries on building and starting orchd. On an upgrade it waits for the old daemon to stop before installing the new one.
- A setup checklist checks git and the claude and codex CLIs and marks routes that cannot run there. Saved hosts connect the first time they are used. A remote daemon keeps running when you quit; reopening reconnects. Tasks from every host count in the Dock badge, tray, Inbox and notices.

## Settings > Orchestration

- Autonomy presets, routes (Claude or Codex, model, effort, strength), tier routing, reviewer, planner, attempts, parallel tasks, budgets and protected paths. A setting that differs from the built-in default is marked, with a reset button.

## For developers: the orchd API

- `orchd mcp` exposes the task tools to any MCP-capable agent.
- `task.create` takes `base`, `parent`, `dependsOn`, `backlog` and `land`; `task.amend` replaces criteria and checks at the next attempt boundary.
- `settings.scopedChecks` (empty by default) skips a command when the diff touches none of its paths; `settings.afterLand` runs commands after a landing.
- CLI: `orchd costs`, `orchd failures` and `orchd gc` (`--dry-run` changes nothing).
