# Install sushiAI

## Download for macOS

The **0.0.6** DMG requires **Apple Silicon (M1 or newer)** and **macOS 13 Ventura or later**. This release does not include a prebuilt Intel package.

1. Open the [0.0.6 release](https://github.com/sushi-killer/sushiAI/releases/tag/v0.0.6).
2. Download `sushiAI-0.0.6-arm64.dmg`.
3. Open the DMG and drag **sushiAI** to **Applications**.
4. Launch sushiAI from Applications, then eject the disk image.

### First launch

This build is ad-hoc signed. It is **not signed with an Apple Developer ID or notarized by Apple**, so macOS may block its first launch.

After attempting to open the app, go to **System Settings → Privacy & Security → Open Anyway** to approve it. See [Apple's instructions for opening an app from an unidentified developer](https://support.apple.com/102445).

### Verify the download

Download `SHA256SUMS.txt` from the release into the same directory as the DMG, then run:

```sh
shasum -a 256 -c SHA256SUMS.txt
```

### Update the app

Open **Settings → Software updates** and choose **Install and restart** when an update is ready. The installer prepares and verifies the new app before closing sushiAI, then replaces it and restarts it. Save file edits first: terminal sessions keep running in the `sushiai` daemon and reattach when the new version opens.

The app must be in a writable folder, such as your personal Applications folder. For manual installation, quit sushiAI and replace the app in Applications using the downloaded DMG.

Version 0.0.1 Alpha does not include an updater. Download and install 0.0.6 manually once; subsequent updates can be installed from Settings.

## Set up your tools

- **Shells:** terminals use your installed shell.
- **Agents:** install and sign in to Claude Code, Codex, Gemini CLI, or Cursor Agent separately. sushiAI launches the CLIs available on your PATH. Codex needs **codex-cli 0.160 or newer** (the release with `--no-daemon`); host setup installs the current Codex on a remote host that has none.
- **Persistent sessions:** nothing to install on This Mac. The app starts the bundled `sushiai` daemon in `~/.sushiai`; sessions survive an app restart or crash and reattach when you open the app again.
- **Files, editing, and Git:** install Python 3 and Git. On macOS, sushiAI uses `/usr/bin/python3`; Xcode Command Line Tools provide these tools. Install them with `xcode-select --install` if needed.
- **Remote project environments:** configure SSH access and install Python 3, Git, and your agent CLIs on the remote host. Verify SSH in Terminal before adding the host in **Settings → Connections**, then choose **Install sushiai** on the host card: sushiAI uploads the matching `sushiai` build over SSH, checks its SHA-256 digest and installs it under `~/.sushiai` on the host. sushiAI prepares project checkouts under `~/sushiai/<slug>` on each host (and on This Mac for a new project): it clones there, pulls an existing checkout of the same repository with a fast-forward only update, and never reuses a folder that is another repository or not a git checkout. A checkout already located elsewhere can still be used, but is marked as a non-standard path. A host you add there receives a project's environment values (tasks and sessions) with no further question; switch "Don't send secrets to this host" on in the project's Hosts settings to keep a project's values off it.

- **Orchestrator settings:** every orchestrator setting is documented in [orchestrator settings](orchestrator-settings.md); the ones without a control are editable under **Settings → Orchestration → Advanced**.

See [sessions and remote hosts](../README.md#sessions-and-remote-hosts) for details. Sessions saved by an older release that used another session backend restore as **Session ended**; choose **Reopen** to start a new one.

## Build from source

On macOS, install **Node.js 22.18+**, npm, Git, and Xcode Command Line Tools.

The `sushiai` daemon, which hosts the orchestrator module, is written in Rust. `npm run build:daemon`, CI and packaging need the Rust toolchain (`cargo`, from [rustup](https://rustup.rs)). The installed app does not: a packaged sushiAI ships the built daemon.

The `sushiai` daemon (sessions, agents, the host proxy) is Rust too. `npm run dev` builds it with `cargo build -p sushiai` before it starts. Packaging needs two more steps, which `npm run package` and `npm run package:dmg` run for you:

- `npm run build:daemon` builds the release `sushiai` for This Mac. The packaged app starts it from its resources and stops with an error that names the missing file when it is not there.
- `npm run build:host` builds the `sushiai` binaries that **Install sushiai** uploads to remote hosts (Linux x86_64 and aarch64, macOS x86_64 and arm64) and writes `target/host/manifest.json`. The Linux targets need [zig](https://ziglang.org) and `cargo-zigbuild` (`brew install zig && cargo install cargo-zigbuild --locked`); the script names any missing tool and installs nothing.

```sh
git clone https://github.com/sushi-killer/sushiAI.git
cd sushiAI
npm ci
npm run dev
```

Build and run the production interface:

```sh
npm run build
npm start
```

Package an `.app` for your Mac's architecture:

```sh
npm run package
```

Build an Apple Silicon DMG:

```sh
npm run package:dmg
```

Build output is written to `release/`. To open a local Apple Silicon build:

```sh
open release/mac-arm64/sushiAI.app
```

`Resources/host` is excluded from electron-builder signing (`mac.signIgnore`): `build:host` ad-hoc signs the two macOS binaries before it hashes them, so `manifest.json` matches the shipped bytes. `npm run package` ends with `node scripts/verify-host-manifest.mjs release/mac-arm64/sushiAI.app`, which fails on any drift. With a Developer ID and notarization, sign those binaries in `scripts/build-host-binaries.mjs` before hashing; do not sign them after packaging.

Packages built with the default configuration are ad-hoc signed, not Apple-notarized. See the [README](../README.md#development) for test commands.
