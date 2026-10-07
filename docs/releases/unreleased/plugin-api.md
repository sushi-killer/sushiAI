## Extensions get Settings tabs and companion programs

Extension API: 1 (additive; no version change)

- An extension can add a tab to Settings. The tab can show a status, a QR code, text and action buttons.
- An extension can name a companion program that is installed outside its folder. It starts only after you approve its path, arguments and permissions in Extensions. An update of the program does not ask again. The approval is consent, not a sandbox: the program keeps your OS rights.
- A companion can read your saved ssh host list when an action asks for it and you granted `hosts.read`.
- The Inbox now takes rows from built-in modules, in the groups Answer, Decide and Review.
