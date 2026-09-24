# sushiAI architecture

A living map of how the pieces fit, drawn in Mermaid so it can be edited next
to the code and reviewed in a diff. Obsidian and GitHub render it as is; VS
Code needs a Mermaid preview extension. Dashed edges and `planned` nodes are
agreed but not built yet. When a change moves a box or an arrow here, update
this file in the same commit.

## 1. System overview

```mermaid
flowchart TB
  owner([Owner])

  subgraph app["Electron app"]
    direction LR
    subgraph renderer["Renderer - sandboxed"]
      direction TB
      shell["Shell - src/app<br/>modes Agent / Code / Chat"]
      panels["Panels<br/>terminal, chat, browser, files, git"]
      orchPanel["Orchestrator panel<br/>src/orchestrator"]
      slots["Extension slots<br/>src/extensions"]
    end
    preload["preload.cjs<br/>window.bridge"]
    subgraph main["Main process - electron/"]
      direction TB
      ipc["ipc/* - app, chat,<br/>terminals, projects"]
      orchSvc["orchestrator.cjs<br/>spawns + proxies orchd"]
      extMgr["extensions/*<br/>manifest validator"]
      herdrIpc["herdr.cjs, connections.cjs"]
    end
    orchPanel <--> preload <--> orchSvc
  end

  subgraph orchd["orchd daemon - Rust, outlives the app"]
    direction LR
    proto["protocol.rs<br/>NDJSON socket + token"]
    engine["engine.rs<br/>task loop, gates, routing"]
    chat["chat.rs<br/>orchestrator chat"]
    harness["harness.rs<br/>claude -p / codex exec"]
    side["classify.rs (Jev)<br/>git.rs (worktrees, commit)<br/>messages.rs (merging)"]
    store[("data dir<br/>tasks/, runs/, settings.json,<br/>decisions.jsonl, chats/")]
    mcp["orchd mcp<br/>task_* tools, stdio"]
    proto --> engine & chat
    engine --> side
    engine & chat --> harness
    engine & chat <--> store
    mcp -->|socket| proto
  end

  subgraph agents["Agent sessions - disposable, one per attempt"]
    direction LR
    taskAgent["Task agent<br/>in its own worktree"]
    reviewer["Reviewer<br/>read-only"]
    orchAgent["Orchestrator agent<br/>Opus, read-only + MCP"]
  end

  herdr[("Herdr daemon")]
  ext[("OpenRouter - Jev")]
  repo[("Git repo<br/>base branch + worktrees")]

  owner --> shell
  orchSvc <-->|socket| proto
  harness --> taskAgent & reviewer & orchAgent
  orchAgent -->|MCP| mcp
  taskAgent -->|Stop hook| proto
  side --> ext
  side --> repo
  herdrIpc <--> herdr
```

## 2. Task lifecycle

```mermaid
stateDiagram-v2
  [*] --> drafting: task.create {repo, request}
  [*] --> queued: task.create {title, goal, criteria, verify}
  drafting --> waiting: planner question
  drafting --> queued: plan parsed
  queued --> running: parallel slot free
  running --> waiting: blocked / protected path /<br/>review gave no verdict /<br/>attempts exhausted
  waiting --> queued: answer
  running --> queued: failed attempt, budget left
  running --> done: gates passed, committed
  running --> stopped: task.stop
  waiting --> stopped: answer "stop"
  stopped --> queued: task.start
  done --> [*]
  note right of done
    archived is a flag, not a state:
    task.archive hides done/stopped/failed/queued-idle tasks
  end note
```

## 3. One attempt: from plan to commit

```mermaid
flowchart TD
  plan["Plan<br/>planner route (Opus) drafts goal, criteria, verify"]
  split["Verify entries that are not shell commands<br/>become review criteria"]
  tier["Tier<br/>Jev: mechanical / standard / hard"]
  impl["Implement<br/>route = tiers[tier]; resume session on retry"]
  rebase["Rebase onto moved base<br/>conflicts go back to the agent"]:::planned
  verify["Verify<br/>task.verify in the worktree<br/>cached by diff + untracked contents"]
  protect["Protected paths<br/>owner approves"]
  review["Review<br/>hard-tier route, sees diff, verify tails,<br/>implementer report"]
  commit["Commit on the task branch"]
  done([done])
  fail["Failure<br/>signature dedup, maybe escalate tier"]
  budget{"Budget left?"}
  triage["Orchestrator triages<br/>continue / reject finding / stop"]:::planned
  ownerQ(["Owner question"])

  plan --> split --> tier --> impl --> rebase --> verify
  verify -->|all exit 0| protect --> review
  review -->|PASS| commit --> done
  review -->|no verdict| ownerQ
  verify -->|non-zero| fail
  review -->|FAIL| fail
  fail --> budget
  budget -->|yes| impl
  budget -->|no| triage --> ownerQ
  triage -.->|continue| impl

  classDef planned stroke-dasharray: 5 5
```

## 4. Roles and models

```mermaid
flowchart LR
  subgraph cheap["Quick decisions"]
    jev["Jev (OpenRouter)<br/>tier, clarity, is it answerable"]
  end
  subgraph strong["Judgement"]
    planner["Planner - claude-opus"]
    reviewerR["Reviewer - tiers.hard<br/>(another harness when the implementer is hard)"]
    orch["Orchestrator chat - claude-opus"]
  end
  subgraph work["Implementation by tier"]
    mech["mechanical - codex"]
    std["standard - claude-sonnet"]
    hard["hard - claude-opus"]
  end
  jev --> work
  planner --> work
  work --> reviewerR
  orch -->|task_create / task_answer via MCP| planner
```

Task agents run with `sandbox: host` today: macOS Seatbelt blocked Electron,
`/tmp` and socket tests, so agents could not screenshot or run the suite. The
worktree is the boundary, stated in the brief.

## 5. What we took from references

```mermaid
flowchart LR
  subgraph refs["References"]
    bm["BridgeMind One<br/>Agent / Code / Chat modes,<br/>MCP task board, routines"]
    oc["OpenClaw<br/>skills with references/,<br/>autoreview, deslop,<br/>ask only consequential decisions"]
    stsa["Stateful Task / Stateless Agent<br/>state on disk, fresh disposable sessions"]
    cc["Claude Code harness<br/>hooks, subagents with models,<br/>headless -p, sandbox, MCP"]
    hd["Herdr<br/>machine-wide session daemon,<br/>agent states"]
  end
  subgraph ours["sushiAI"]
    modes["Shell modes Agent / Code / Chat"]
    orchdN["orchd: task store + socket API,<br/>orchestrator agent over MCP"]
    skills[".agents/skills + role agents<br/>implementer, reviewer, qa, design-critic"]
    sessions["Terminal panels over Herdr"]
  end
  bm --> modes
  bm --> orchdN
  stsa --> orchdN
  cc --> orchdN
  oc --> skills
  cc --> skills
  hd --> sessions
```

## 6. Planned next

```mermaid
flowchart LR
  a["Rebase step before verify"]:::planned
  b["Exhausted-attempts question<br/>triaged by the orchestrator"]:::planned
  c["Parallel tasks on one feature base<br/>(task.create base + merge order)"]:::planned
  d["Base-branch field in the new-task form"]:::planned
  a --> c
  classDef planned stroke-dasharray: 5 5
```
