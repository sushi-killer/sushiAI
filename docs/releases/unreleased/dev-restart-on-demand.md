# Dev restarts Electron on demand

While running `npm run dev`, editing a file under `electron/` no longer kills
the app mid-work. The desktop mascot shows a "sushiAI core - Core updated -
restart?" notice instead; click **Restart** when you are ready and sushiAI
closes normally and comes back with the new code. Dismiss the notice to keep
working on the old code. Renderer edits still hot-reload as before.
