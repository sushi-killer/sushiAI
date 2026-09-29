## The orchestrator chat proposes tasks and asks questions

- When the orchestrator proposes a task, its reply now carries a structured draft (title, goal, criteria, dependencies, tier) that the owner can confirm, instead of a task described only in prose.
- When it needs a choice, it asks structured questions with options; a reply may carry questions alone.
- The draft and questions are taken out of the message text, so the conversation reads as plain prose; the session keeps the latest proposed task until it is cleared.
- Clearing the chat also drops its draft, and the new `chat.clearDraft` method drops only the draft and keeps the messages.
- The protocol additions are additive: chats saved before this change load unchanged, and messages without a draft or questions look exactly as before.
