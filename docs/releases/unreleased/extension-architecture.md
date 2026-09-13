## Extensions: sushiAI is now plugin-extensible

- A new extension contract lets a declarative `manifest.json` add its own
  navigation entries, panes, toolbar actions and commands to the app - no
  executable code ships with an extension, only UI and data description.
- Extensions get their own persisted state, scoped per pane, per project, or
  globally, and a singleton surface (like a settings page) reveals its
  existing pane instead of opening a duplicate.
- The extension manifest's compatibility version (`apiVersion`) is tracked
  independently of sushiAI's own release version, so a future third-party
  extension author has a stable contract to build against even as the app
  keeps shipping.
- The previous ad-hoc Tasks/Links demo data has been replaced by a Probe
  fixture built specifically to exercise everything the contract allows.
