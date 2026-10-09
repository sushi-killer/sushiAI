## Extensions

- A companion extension can show a list of rows in its Settings tab, each with a status and an optional button.
- A companion that declares the `hosts.exec` permission can run commands on your saved ssh hosts through the app. Every command needs your confirmation in a card that shows the extension, the host and the exact command. Deny is the default and Allow needs a click. Companions with `hosts.read` also learn when the host list changes. Extension API stays at version 1.
