"use strict";

// The hosts list a companion action may ask for (permission `hosts.read`).
// It reads an injected getter over the saved connection profiles, so this file
// knows nothing about where profiles live. Command connectors have no shell,
// so only ssh profiles are listed. Profiles hold no secrets.

function listSshHosts(getProfiles) {
  const profiles = typeof getProfiles === "function" ? getProfiles() : [];
  return (Array.isArray(profiles) ? profiles : [])
    .filter(
      (profile) =>
        profile &&
        profile.id !== "local" &&
        profile.connector?.kind !== "command",
    )
    .map((profile) => ({
      id: String(profile.id),
      name: String(profile.name ?? profile.host),
      host: String(profile.host),
      ...(Number.isInteger(profile.port) ? { port: profile.port } : {}),
    }));
}

module.exports = { listSshHosts };
