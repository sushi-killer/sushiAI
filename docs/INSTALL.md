# Install sushiAI

## Download for macOS

The **0.0.4** DMG requires **Apple Silicon (M1 or newer)** and **macOS 13 Ventura or later**. This release does not include a prebuilt Intel package.

1. Open the [0.0.4 release](https://github.com/sushi-killer/sushiAI/releases/tag/v0.0.4).
2. Download `sushiAI-0.0.4-arm64.dmg`.
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

Open **Settings → Software updates** and choose **Install and restart** when an update is ready. The installer prepares and verifies the new app before closing sushiAI, then replaces it and restarts it. Save file edits first: local terminals stop during the restart, while Herdr sessions continue running.

The app must be in a writable folder, such as your personal Applications folder. For manual installation, quit sushiAI and replace the app in Applications using the downloaded DMG.

Version 0.0.1 Alpha does not include an updater. Download and install 0.0.4 manually once; subsequent updates can be installed from Settings.

## Set up your tools

- **Shells:** local terminals use your installed shell.
- **Agents:** install and sign in to Claude Code, Codex, Gemini CLI, or Cursor Agent separately. sushiAI launches the CLIs available on your PATH.
- **Persistent sessions:** install and start [Herdr](https://github.com/ogulcancelik/herdr). Local terminals do not require it.
- **Files, editing, and Git:** install Python 3 and Git. On macOS, sushiAI uses `/usr/bin/python3`; Xcode Command Line Tools provide these tools. Install them with `xcode-select --install` if needed.
- **Remote workspaces:** configure SSH access and install Herdr, Python 3, Git, and your agent CLIs on the remote host. Verify SSH in Terminal before adding the host in **Settings → Connections**.

Use `~/.config/herdr/herdr.sock` for the default Herdr socket or `~/.config/herdr/sessions/<name>/herdr.sock` for a named session. See [connecting to Herdr](../README.md#connect-to-herdr) for details.

## Build from source

On macOS, install **Node.js 22.18+**, npm, Git, and Xcode Command Line Tools.

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

Packages built with the default configuration are ad-hoc signed, not Apple-notarized. See the [README](../README.md#development) for test commands.
