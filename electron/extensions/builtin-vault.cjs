// The built-in Vault module's manifest: the one owner of this object. It adds
// one Settings tab (the token); the per-project block is a module section.
const VAULT_MANIFEST = {
  id: "builtin.vault",
  name: "Vault",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  icon: "key-round",
  description:
    "Reads a project's values on This Mac from HashiCorp Vault, read-only.",
  contributions: {
    surfaces: [
      {
        id: "settings",
        title: "Vault",
        allowedHosts: ["settings.page"],
        defaultHost: "settings.page",
        instancePolicy: "singleton",
        stateScope: "global",
        view: { kind: "core", viewId: "vault.settings" },
      },
    ],
    navigation: [],
    actions: [],
    commands: [],
  },
};

module.exports = { VAULT_MANIFEST };
