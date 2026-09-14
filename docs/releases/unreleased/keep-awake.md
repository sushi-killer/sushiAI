### Keep this Mac awake while sushiAI is open

Settings → General now has a **Sleep** checkbox that stops the Mac idle-sleeping
while sushiAI is running, so a long agent turn is not cut short halfway. It
replaces leaving `caffeinate` running in a terminal.

The display still sleeps normally — only system suspension is held, the same
behaviour `caffeinate -i` gives. The setting is off by default, is remembered
between launches, and the blocker is released as soon as you uncheck it or quit
the app.
