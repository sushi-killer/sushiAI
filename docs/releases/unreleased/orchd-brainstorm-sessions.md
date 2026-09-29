## Brainstorm sessions in orchd

- Every orchd chat session now has a `kind`: `chat` (the orchestrator chat, the default) or `brainstorm`.
- A brainstorm session always starts empty and refines a feature idea with the owner: it asks one focused question per reply with 2-4 answer options and ends every reply with a task draft (title, goal, criteria, dependencies, tier guess), which the session keeps up to date.
- A brainstorm agent can look at tasks, settings and repo notes but can never create, start or change tasks; its tool bridge is read-only.
- Chat and brainstorm turns run side by side: a brainstorm turn never makes the chat busy, and the reverse. Task questions are still answered only in the chat kind.
- `chat.new`, `chat.get`, `chat.list`, `chat.send`, `chat.switch`, `chat.clear` and `chat.cancel` take an optional `kind`; without it they mean `chat`, so existing callers never see a brainstorm. Switching to a session of the other kind is refused.
- Session summaries in `chat.list` now carry `kind`, `updatedAt` and `messageCount`, and the `chat` event carries `kind`.
- The additions are additive: stores saved before this change load with every session as `chat`.
