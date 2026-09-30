## Chat, Brainstorm and Plan in one conversation

- The orchestrator chat composer has a Chat / Brainstorm / Plan switch. The mode is chosen per message inside one conversation and restored when the session is reopened.
- Brainstorm asks one focused question per reply (answer options as chips), writes the understanding back, weighs approaches and, once the design is agreed, proposes tasks. Plan splits a goal into small, ordered, independently verifiable tasks with acceptance criteria, dependencies and a tier guess. Both only look at the repository and tasks; they never create or start tasks themselves.
- A reply that proposes tasks shows a Proposed tasks card: tick the rows to keep, then Create one row, Create all checked rows or Add to plan. Created rows show as queued and open their task; unchecked rows read as skipped, and the card reads the same after a reload.
- The separate Brainstorm screen is gone. The Plan page's Brainstorm button opens Chat on a new session in Brainstorm mode; earlier brainstorm sessions load as ordinary chat sessions with their messages.
- orchd: `chat.send` takes an optional `mode` (`chat`, `brainstorm`, `plan`); the `sushi-draft` block may carry `tasks`, stored as the reply's `proposal`; the new `chat.createProposal {repo, messageId, indices, skip?, backlog?}` creates the chosen rows in dependency order. The `kind` parameter of the `chat.*` methods, the session draft and `chat.clearDraft` are removed.
- The Brainstorm and Plan prompts adapt obra/superpowers (MIT), credited in THIRD_PARTY_NOTICES.md.
