# The orchestrator can be turned off, and only runs when used

The orchestrator can now be turned off from Extensions. While it is off, sushiAI
does not start or contact the orchestrator daemon or its SSH hosts, and the
Inbox, the menu bar badge, Settings and the panel picker look and behave as they
did before the orchestrator existed. The choice is remembered across restarts,
and turning it back on needs no restart. An Orchestrator panel left in a saved
layout shows a short note instead of loading.

The orchestrator no longer starts when sushiAI launches. A daemon that is
already running is picked up so its task notices keep coming; otherwise the
first thing that needs it (opening the Orchestrator panel, creating a task, an
answer from the mascot) starts it, and saved SSH hosts connect the first time
they are used. Quitting sushiAI now stops the local daemon it started; daemons
on SSH hosts keep running and are only disconnected.
