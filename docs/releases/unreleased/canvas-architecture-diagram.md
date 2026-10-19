## Architecture diagrams in Preview

- A JSON Canvas file (`.canvas`) opens in Preview as one diagram. Every part is a block with a shape (box, pill, diamond, cylinder, person or circle), an icon, a caption and an optional step number. Use it for an architecture, an order of steps of an agent or a skill, or a change as-is / to-be: each part gets a delta (new, changed, exists or removed), and the marks on the board show the change.
- The agent draws and changes the diagram from the terminal. You build it too: right-click the board (or use the + button on it) to add a component, boundary or note where you clicked; drag, connect, rename and delete parts; move and resize boundaries and drop components into them; edit everything in Inspect.
- Drag on an empty spot of the board to select several parts at once (hold Shift to add more, Cmd or Ctrl+A for all), then move them together with the mouse or the arrow keys, change what they share in Inspect, or delete them. Pan with two fingers, Space and drag, or the middle button.
- Accept or reject parts the agent proposed, and comment a selection to send it to the agent.
- Edits from both sides merge into the same file. The agent gets one note to reread the file after your edits, not one per edit.
- The file is plain JSON Canvas, so it also opens in Obsidian and other JSON Canvas tools. The new, changed and removed marks are colours there too (4, 3 and 1). Shapes, icons, step badges and the proposed state live in a `sushiai` field that other tools may drop when they save.
