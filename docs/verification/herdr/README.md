# Herdr verification records, 2026-10-01

[The implementation and measurement report](../../HERDR-STABILITY.md) describes
the methodology, final before/after table, acceptance checks and limitations.
The runtime is the official Herdr 0.8.2 release. The owner excluded a Herdr
patch; these records contain no fork evidence or patched-runtime gain claim.

- [Local before](local-before.json) and [local after](local-after.json): real
  hidden Electron, direct sidebar DOM timestamps and counted full-snapshot RPCs.
- [SSH before](ssh-before.json) and [SSH after](ssh-after.json): real encrypted
  SSH over loopback; no WAN performance claim.
- [Lifecycle](lifecycle-local-ssh.json): 20 independent launches and repeated
  operation IDs, environment, worktrees, preparation retry and process survival.
- [Official release RPC](release-rpc.json): daemon and CLI compatibility checks.
- [Release metadata](release-metadata.json): primary upstream artifact digests.
- [Managed SSH installation](managed-ssh-install-live.json): clean remote
  official download, checksum verification, repeat verification and independent
  daemon/CLI compatibility. Its local portion uses previously verified official
  asset bytes, as explicitly recorded; the separate local record proves a fresh
  online download.
- [Managed local installation](managed-install-live.json): successful official
  asset download, SHA-256 verification and compatibility.

The after app records are the final repeat on the verified working tree, with
built asset hashes. The baseline is `113f576`; the source label in after records
identifies implementation commit `ae6de54`, whose production source was unchanged during measurement. Later changes update evidence/the UI fixture and preserve main `9f45d06` Git-over-SSH recovery; that integration does not alter the Herdr snapshot or terminal-flow modules. The raw timings remain tied to their recorded source commits. The compact
[measurement manifest](../../measurements/herdr-stability.json) hashes these raw
records and the implementation files.

Individual timings include Playwright locator and hidden-window animation
callbacks as well as direct DOM timestamps. Slower locator or animation samples
have not been discarded; physical paint within 500 ms remains unverified.
The original SSH terminal fixture could not attach its stream in the baseline;
its sidebar tunnel and process-preservation checks worked. No baseline SSH
terminal-rendering improvement is quantified.
