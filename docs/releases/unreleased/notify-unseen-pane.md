## Notices for panes you cannot see

- An agent that waits for you now raises a notice even when sushiAI is the focused window, as long as the waiting pane is not on screen. In Chat or Agent mode, or with a pane from another project open, the waiting pane counts as not on screen.
- A notice is skipped only when you are already looking at that pane.
- A pane that keeps waiting is reminded at 5, 10 and 20 minutes, then at 50, 110, 230 minutes and so on (the gap doubles). Opening the pane ends the reminders until it waits again.
- Reminders due at the same time arrive as one notice ("N panes still waiting"); a click opens the pane that has waited longest. Panes on hidden hosts are skipped.
