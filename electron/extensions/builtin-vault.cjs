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

/** The skill this extension ships: its name, its file in this folder. */
const VAULT_SKILL = {
  name: "sushiai-vault",
  file: "vault-skill.md",
  // Vault values never go to an SSH host, so neither does the skill.
  hosts: "local",
};

module.exports = { VAULT_MANIFEST, VAULT_SKILL };
