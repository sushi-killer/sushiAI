## Diagram editor fixes

- The agent again gets one note to reread a diagram after a stretch of your edits. A poll that read your own save before the save's reply arrived looked like an agent write, so a drag could send a note on every step.
- A block's title and subtitle wrap instead of being cut off. A block grows to fit its text, up to 320 px wide, then wraps and grows taller.
- Select a block and drag its corner to resize it. The width you choose stays and the text wraps inside it; the block is marked `"sushiai": {"size": "fixed"}` in the file. A block or note you did not touch keeps the width and height the file gives it.
- A note can hold several lines: in Inspect, Enter adds a line and Cmd or Ctrl+Enter saves.
- A link's label no longer sits on an arrowhead, on another label or under a block. Set a link's direction in Inspect: forward, reverse, both or none.
