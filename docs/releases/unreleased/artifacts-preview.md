## Artifacts

- An agent can now show you what it wrote in a Preview pane that opens to the right of its own pane, split 50/50. Your focus stays in the terminal. Preview shows Markdown documents and plans, any HTML file (reveal.js decks, Mermaid diagrams, reports), SVG, images and PDF, and updates as the agent edits the file. It works on SSH hosts too.
- Select text, or point at an element of a deck or diagram, to leave a comment. **Send** delivers all your comments to the agent as one message.
- A plan has a **Start task** button. You pick Claude Code or Codex, which opens a new pane in a new worktree and starts with `/goal <plan>`, or the orchestrator. Plans show whether the agent checked them for contradictions ("Verified").
- sushiAI installs the `sushiai-artifacts` skill for Claude Code and Codex, on this Mac and on every SSH host, so any agent started from sushiAI knows how to open a Preview. You can turn Artifacts off in Extensions.
- Extensions: a built-in extension surface can now open beside an agent pane.
