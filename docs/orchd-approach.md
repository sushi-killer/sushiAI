# orchd approach

orchd runs agent work on any repository. These rules decide where a change to
orchd goes: into code, into a prompt, or into an agent turn. Read them before
you change `crates/sushiai-orch`.

## Terms

| Term         | Meaning                                                                     |
| ------------ | --------------------------------------------------------------------------- |
| agent turn   | One LLM run with a fixed input and a structured (JSON) output.              |
| sandbox turn | An agent turn in a clean checkout that can run commands, never push.        |
| gate         | Deterministic code that allows or refuses a step.                           |
| fast path    | Deterministic code that settles the certain cases before any agent turn.    |
| judgement    | A decision that needs reading text, code or output: whose fault, what next. |

## Rules

1. **Judgement is an agent turn.** Code does not read test output, error text
   or question text to decide what it means. An agent turn reads it and
   returns typed fields (`verdict`, `cause`, `target`, `risk`,
   `default_option`, `evidence`). Code acts on the fields.
2. **Code keeps four jobs only:** gates (permissions, owner-only decisions,
   protected-file restore, budgets, branch rules), bookkeeping (task state,
   leases, retries, timeouts), fast paths for certain cases (exit code 0,
   identical output, timeout, a file absent on the base), and the socket API.
3. **An agent investigates, code does not parse.** When the agent needs
   evidence, give it a sandbox turn: it can rerun a check on the base, read
   the diff and compare runs itself. Do not write a parser per test runner or
   per tool.
4. **No string matching on text that an LLM or a tool wrote.** No keyword
   lists, no substring rules, no regex over output. If code needs a fact, the
   producer emits it as a typed field.
5. **No state in English text.** Task state lives in typed fields. Decision
   lines are for the owner to read, never parsed back.
6. **An agent can only add caution.** An agent field may escalate to the owner
   or refuse a step. It never removes a gate. Owner-only decisions, permission
   checks and protected-file restore stay code.
7. **Fail safe on unknowns.** A rule that knows some ecosystems (npm, cargo)
   must not be the only guard. Decide by what the task created, not by a list
   of known tools.
8. **Generic.** No repo names, file names, tool names or runner names in
   orchd code or prompts. Repo facts come from the repo (its instructions, its
   checks) or from an agent that reads it.
9. **A live failure is a design question first.** Before a point fix, ask:
   which judgement did code try to make here? Move that judgement to an agent
   turn. A point fix that adds a special case needs a note why an agent turn
   does not fit.

## Before you add code to orchd

- Is this a gate, bookkeeping, a fast path or API? If not, it is an agent turn.
- Does it read text that an LLM or a tool wrote? Then it is an agent turn.
- Does it add an item to a list of tools, runners, keywords or error strings?
  Then it is an agent turn.
- Does a prompt already ask an agent to decide the same thing? Then remove one
  of the two.
