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
    engine["engine/<br/>task loop, gates, routing"]
    chat["chat.rs<br/>orchestrator chat"]
    audit["audit.rs<br/>repo.audit, read-only"]
    insights["timeline.rs<br/>task.timeline, failures.catalogue"]
    evolution["evolution/<br/>signals when a task ends,<br/>clusters, proposals, measurement"]
    evolveCli["orchd evolve<br/>evolution.run / evolution.adopt"]
    harness["harness.rs<br/>claude -p / codex exec"]
    side["classify.rs (Jev)<br/>git.rs (worktrees, commit)<br/>messages.rs (merging)"]
    store[("data dir<br/>tasks/, runs/, settings.json,<br/>decisions.jsonl, chats/, audits/,<br/>evolution/ (signals.jsonl, detected.jsonl,<br/>proposals/)")]
    mcp["orchd mcp<br/>task_* tools, stdio"]
    proto --> engine & chat & audit & insights & evolution
    engine --> side
    engine -->|task done or failed| evolution
    evolution -->|append signals| store
    evolution -->|evolution.run: proposer run, read-only| harness
    evolveCli -->|socket| proto
    engine & chat & audit --> harness
    engine & chat & audit & evolution <--> store
    insights --> store
    mcp -->|socket| proto
  end

  subgraph agents["Agent sessions - disposable, one per attempt"]
    direction LR
    taskAgent["Task agent<br/>in its own worktree"]
    reviewer["Reviewer<br/>read-only"]
    auditor["Auditor<br/>read-only, in the repo's checkout"]
    proposer["Proposer<br/>read-only, in the cluster's main repo"]
    orchAgent["Orchestrator agent<br/>Opus, read-only + MCP"]
  end

  herdr[("Herdr daemon")]
  ext[("OpenRouter - Jev")]
  repo[("Git repo<br/>base branch + worktrees")]
  rtk[("rtk CLI")]

  owner --> shell
  orchSvc <-->|socket| proto
  harness --> taskAgent & reviewer & auditor & proposer & orchAgent
  orchAgent -->|MCP| mcp
  taskAgent -->|Stop hook| proto
  taskAgent -->|rtk rewrite hook<br/>leanOutput, no socket| rtk
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
  drafting --> running: plan split into subtasks<br/>(the task becomes their parent)
  queued --> running: parallel slot free,<br/>every dependsOn task done
  queued --> waiting: a dependsOn task or child<br/>ended failed/stopped
  running --> waiting: blocked / protected path /<br/>review gave no verdict /<br/>attempts exhausted
  waiting --> queued: answer
  running --> queued: failed attempt, budget left
  running --> done: gates passed, committed<br/>(a subtask: landed on its parent's branch;<br/>a parent: every child landed, its checks passed)
  running --> stopped: task.stop
  waiting --> stopped: answer "stop"
  stopped --> queued: task.start
  done --> [*]
  note right of done
    archived is a flag, not a state:
    task.archive hides done/stopped/failed/queued-idle tasks
  end note
```

A task graph is state orchd keeps, never a split it decides: the planner
may answer a top-level request with `subtasks` (keys, requests, `dependsOn`
between keys), or the orchestrator agent builds one with `task.create
{parent, dependsOn}`. A parent runs no implement attempt; each child
branches from the parent's branch, is drafted and run as a normal task, and
starts implementing only once its `dependsOn` tasks are done (it syncs to
the parent's head first, so it sees their work). A finished child lands
through a per-parent merge queue: its work is carried onto the parent's
current head, verify runs again when that head moved, and the parent's
branch fast-forwards to the child's commit; a conflict or failing verify is
an ordinary failure the agent retries. When every child has landed, the
parent runs its own verify and final checks on its branch (with
`groundedChecks`, its checks were baselined before the first child started and
its gated and held-out checks run once here) and is done; its
cost is its plan plus its children. When something a task waits for ends
`failed`/`stopped`, the task waits with a question: retry the dependency,
drop it, or stop.

## 2b. Evolution flow

```mermaid
flowchart LR
  ended([task done or failed]) --> detect[detect signals]
  detect --> cluster[cluster by cause]
  cluster --> propose["propose<br/>read-only proposer run"]
  propose --> gate{gate}
  gate -->|weakens a check or protected path,<br/>targets AGENTS.md or memory| rejected([stored rejected, with a reason])
  gate -->|passes| proposed([proposed])
  proposed -->|repo track: Approve| task[task runs] -->|done| adopted([adopted])
  proposed -->|harness track: orchd eval run,<br/>evolution.adopt| adopted
  adopted --> measure[measure later tasks] -->|metric regressed| revert([revert_suggested])
```

## 3. One attempt: from plan to commit

```mermaid
flowchart TD
  plan["Plan<br/>planner route (Opus) drafts goal, criteria, verify"]
  split["Verify entries that are not shell commands<br/>become review criteria"]
  baseline["Baseline (variant.groundedChecks), before attempt 1<br/>planner's checks and held-out check run on the base;<br/>pass = not grounded, exit 126/127 = env, fail = gated;<br/>worktree restored afterwards"]
  tier["Tier<br/>planner's tier (variant.plannerTier), else Jev"]
  impl["Implement<br/>route = tiers[tier]; retry resumes the session,<br/>or starts fresh with handoffs (variant.retryMode)"]
  stall["Stall watchdog<br/>no output for variant.stallTimeoutSecs -> kill"]
  rebase["Carry onto moved base<br/>conflicts go back to the agent"]
  verify["Verify<br/>task.verify in the worktree<br/>cached by diff + untracked contents"]
  gated["Gated checks<br/>baseline-fail checks must now pass"]
  heldout["Held-out check<br/>hidden from the implementer; failure kind heldout,<br/>command never in a brief or detail"]
  impossible["sushi-impossible block in the reply<br/>criterion cannot be met as written"]
  protect["Protected paths<br/>owner approves"]
  review["Review<br/>hard-tier route, sees diff, verify tails,<br/>implementer report"]
  final["Final checks<br/>task.finalVerify, once, after review passes"]
  baserun{"Same command on the base?<br/>throwaway worktree on base sha,<br/>cached per base + command"}
  commit["Commit on the task branch"]
  done([done])
  fail["Failure<br/>signature dedup, maybe escalate tier"]
  budget{"Budget left?"}
  ab[["orchd ab<br/>metrics per variant"]]
  triage["Orchestrator triages, max 2 per task<br/>continue / reject finding / escalate"]
  ownerQ(["Owner question"])

  plan --> split --> baseline --> tier --> impl --> rebase --> verify
  impl -.- stall
  stall -->|stalled| fail
  verify -->|all exit 0| gated -->|pass| heldout -->|pass| protect --> review
  gated -->|fails, kind verify| fail
  heldout -->|fails, kind heldout| fail
  impl -.- impossible
  impossible -->|"drop criterion / retry / stop"| ownerQ
  ownerQ -->|drop criterion or retry| impl
  ownerQ -->|"drop this check"| commit
  land["Subtask: land on the parent's branch<br/>one at a time per parent; carry onto its head,<br/>verify again if it moved, fast-forward"]
  review -->|PASS| final -->|all exit 0| commit --> done
  final -->|all exit 0, subtask| land --> done
  final -->|non-zero| baserun
  baserun -->|passes on base| fail
  baserun -->|also fails: retry after base is fixed / drop this check / stop| ownerQ
  land -->|conflict / verify fails| fail
  review -->|no verdict| ownerQ
  verify -->|non-zero| fail
  review -->|FAIL| fail
  fail --> budget
  budget -->|yes| impl
  budget -->|no| triage --> ownerQ
  triage -->|continue| impl
  commit -.-> ab

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
  d["Base-branch field in the new-task form"]:::planned
  classDef planned stroke-dasharray: 5 5
```
