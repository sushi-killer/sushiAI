# Agents integration

Status: included in 0.0.3; the remaining integration gaps are documented below.

## Design

The Agents view uses a provider registry in the Electron main process. A
provider owns discovery, conversations, execution, interaction requests and
capability add-ons. React consumes provider-neutral descriptions and transcript
items. Provider protocol names, credentials and process management never cross
the preload boundary.

The Hermes provider uses the installed runtime's native `hermes serve` backend:
authenticated HTTP for resources and profile-scoped WebSocket JSON-RPC for
interactive sessions. Hermes remains the authority for profiles, conversations,
memory, skills and settings. sushiAI must not write Hermes databases directly,
scrape terminal output or embed Hermes Desktop. Its own headless process is
owned separately from the messaging gateway and the desktop application.

Each profile has a dedicated lazily started backend: Hermes's WebSocket route
does not interpret a `profile` query parameter. Session-scoped RPCs additionally
carry their runtime binding; HTTP operations use the route's explicit profile
query or body field. Global RPC configuration is therefore isolated by process.
The transport runs inside Electron main, not a separate credential-bearing
worker. Closing a view detaches presentation only. Quitting sushiAI closes its
owned runtimes; the independent Hermes messaging gateway remains responsible
for its own background jobs. An optional, separately owned native Desktop backend can execute automatic
schedules while sushiAI is open.

Providers and capability add-ons are statically registered modules. An alternate
engine implements the same contract without changing the conversation UI. An
unsupported capability is explicitly unavailable, never simulated.

## Acceptance checklist

- [x] Discover both existing Hermes agents with stable profile identity.
- [ ] Open the canonical agent conversation and other conversation tabs; create,
      rename, archive, delete, search, resume and paginate conversations.
- [ ] Stream text, actual available reasoning, tool arguments/results/progress,
      subagents, todos, usage and errors in their original order.
- [ ] Show actual memory writes, self-improvement and skill activity; keep full
      details accessible after reload where the engine persists them.
      The primary experience is agent activity and notifications, not manual memory
      editing. Distinguish committed tool writes from staged approvals and failures.
      Show native `review.summary` notifications even after the main answer ends.
- [ ] Send, interrupt, recover after disconnect/restart, and preserve background
      runs when switching tabs or modes without duplicate prompts.
- [ ] Handle approvals, clarification, secret/password requests and desktop tool
      callbacks explicitly; never auto-approve or leave a tool silently blocked.
- [ ] Manage agent identity/instructions, models, reasoning and tool permissions.
- [ ] Read and edit memory, manage skills, tools and MCP integrations.
- [ ] Manage schedules, projects/files, attachments and supported voice workflows.
- [ ] Expose remaining Hermes workflow capabilities through native add-on panels;
      audit against the installed Desktop feature inventory before completion.
- [ ] Keep authentication in main; scope every operation and event by provider,
      agent and conversation; validate IPC and close only owned resources.
- [ ] Match sushiAI styling, support narrow layouts and keyboard navigation,
      retain Code, Chat and the original Dashboard unchanged.
- [ ] Verify deterministic protocol/UI tests plus real read-only discovery and
      history on both agents and controlled disposable execution sessions.
- [ ] Deliver a runnable local build for testing, without commits, pushes or a
      public release until requested.

## Verification policy

Use synthetic fixtures for automated tests and screenshots. Do not commit real
conversations, memory, credentials, profile paths or logs. Live smoke checks
report counts, shapes and outcomes rather than private text. Changes to existing
agent configuration and memory require an explicit user operation; development
tests use disposable records. Never infer an agent event from an animation or
timer, and never claim feature parity based only on a successful text reply.

Native Hermes startup and some list endpoints may perform maintenance or
configured auto-archival. Read-only verification must account for these effects;
mutation coverage uses isolated profile homes, not just temporary conversations.
The provider never retries an ambiguous prompt submission automatically. Hermes
may continue a crashed turn itself when its `desktop.auto_continue` policy is
enabled; sushiAI honors that native policy and reports the continuation. Resume
must reconcile native event sequence/epoch changes against durable history.

## Agent activity

The Activity panel and application notifications report confirmed memory writes,
skill changes and native background review summaries. Tool starts, staged changes,
failed writes and claims in an assistant answer do not produce a saved-change
notification. A review summary can arrive after the main response has finished.
Explicit duplicate and unchanged results are suppressed. Batch memory results
report completed operations, not an inferred count of new memories: Hermes can
silently skip duplicate entries within a successful batch.
Hermes's own review and memory notification settings still control whether it
performs and reports a background review.

The latest 500 observed events are saved in sushiAI's private application data
directory and remain available after restart. This is an observation journal for
sessions connected through sushiAI, not an audit of every Hermes process. It does
not infer activity in independent Desktop or messaging sessions. Manual memory
and SOUL editing is a separate optional resource panel.

## Local verification — September 7, 2026

- The full regression suite passes: 137 tests, including provider isolation,
  streamed events, request routing, transport recovery and journal persistence.
- The production renderer build passes.
- `node scripts/agents-smoke.mjs` passes against synthetic data in Electron:
  transcripts, activity, instructions, approval routing, batch clarification with
  an interrupted second answer and a successful retry, history search, unsent
  draft restoration across mode switches and a 600×440 layout.
- `node scripts/hermes-native-smoke.cjs` uses the installed Hermes runtime with
  an isolated temporary home and deterministic local model. Real native memory
  and skill tools write disposable files; a real background review performs a
  further memory write. All three activity kinds are observed, including the
  review summary after the main answer. No real profile credentials or data are
  copied into the test. An empty conversation survives a full runtime restart.
  Crashing the owned test backend both while idle and during a model request
  restores saved history without submitting the interrupted prompt again when
  native automatic continuation is disabled in the disposable profile.
  A native batch clarification verifies both question IDs, partial acknowledgement
  and the actual tool result containing both answers. Request-specific approval
  choices, expired responses and resumed partial answers have regression coverage.
- Both installed profiles were discovered and their existing histories and
  resource lists read locally. Existing profile mutations remain untested.

The broader unchecked items above remain in progress. These checks do not yet
establish full Hermes Desktop feature parity. Recovery of all pending interaction
kinds, reconnect replay gaps during a live turn, and other unchecked capabilities
still require further coverage.
This verification predates the 0.0.3 release, which includes this integration.

### Connections module

The provider-neutral Connections panel uses explicit MCP add-on operations. It
lists configured servers without environment values, accepts HTTP or stdio
configuration, probes tools, toggles servers and removes connections. OAuth
start/status/cancel use native Hermes flows scoped to the owning profile; flow
identifiers remain in main. Browser authorization is an explicit user action.
Changes apply to new agent sessions according to the native Hermes contract.

The isolated native smoke test verifies create, list, successful tool discovery,
disable and removal against a local synthetic MCP server. Electron checks cover
the add form, tool results and compact layout. OAuth routing and cross-profile
isolation are tested with protocol fixtures. The full OAuth round trip is also
verified by `node scripts/hermes-oauth-smoke.cjs`: a disposable local identity
provider validates PKCE, invalid callback state is rejected, authenticated tools
are discovered, and a separate sign-in can be cancelled without a token exchange.
No real account or external service is involved.

Tool access supports exact names or wildcard filters, an explicit empty allow
list, exclusions, prompts and resources. Native tests verify saving an empty list
and clearing it; UI tests cover editing the policy. Background catalog bootstrap
against a real installer remains to be verified before this module is considered
complete.

Existing connections have an editor for their URL or command/arguments, plus
explicit environment replacements and removals. Saved credential values never
enter the form. Authentication, tool policy, custom fields and other connections
are preserved by main and saved through Hermes's validated MCP map endpoint.
A profile- and server-scoped revision rejects an entry changed since opening the
editor. Hermes does not expose an atomic conditional map update, so a concurrent
external writer between the final read and save is not covered by that revision;
configuration edits within this provider are serialized. Changing transport or
authentication type and rotating an HTTP bearer token are not yet exposed here.

### Universal workspace controls

Workspace settings expose one compact, provider-neutral controls table. The
first harness is Claude Code: MCP server names come from the current project's
`.mcp.json`, the user's `~/.claude.json`, and the per-project Claude state.
A toggle preserves every server definition and writes only Claude Code's native
per-project `disabledMcpjsonServers` or `disabledMcpServers` list. This includes
saved Claude.ai connector choices when Claude Code has exposed their display name.

Claude Code plugins are read from `enabledPlugins` in user, project and local
settings plus the installed plugin catalog. Per-project changes are written to
`.claude/settings.local.json`, so a personal disable does not rewrite shared
`.claude/settings.json`. Values from server environments, plugin settings and
credentials are never sent to the renderer. Changes affect new Claude Code
sessions; existing sessions must be restarted. New workspace creation discovers
the same plugin catalog and offers a multi-select list of plugins to start
disabled. The Stats tab correlates the local skill catalog with recent Claude
skill events by project; it is a review signal rather than a complete historical
audit, and SSH analytics is intentionally deferred until the remote event
catalog is exposed. SSH workspaces use the same validated protocol through the
selected SSH host. The table is grouped by harness so Codex and other agents can
add controls without another modal.

`node scripts/hermes-settings-smoke.cjs` verifies native HTTP and command edits,
credential and tool-policy preservation, environment replacement/removal and stale
revision rejection in a disposable profile. Electron coverage exercises opening
and submitting the edit form. No real profile configuration is changed.

The catalog panel now lists native entries, shows URL/command/bootstrap before
installation, accepts only entry-declared credentials and tracks background
installations by their actual exit status. Job IDs stay scoped in main. Native
verification discovered 65 entries and installed then removed a disabled,
credential-free HTTP entry in the disposable profile. No third-party bootstrap
command was executed in this check. Electron fixtures cover catalog selection
and installation; protocol tests cover credential validation and job isolation.

### Schedules

The schedule panel supports manual execution, observed completion or failure,
run history and opening a run as a conversation. Manual execution is tracked in
main independently of the active panel; repeated clicks during an active run
reuse its status. The native synchronous trigger has a bounded ten-minute HTTP
deadline; an ambiguous timeout is reported as unknown and never retried
automatically. Interval, cron and one-shot forms retain their native expression.
Optional model/provider/delivery values can be cleared.

The native isolated test creates a recurring schedule, pauses it, manually fires
it through the real Hermes scheduler with a local synthetic model, verifies a
run history record, clears overrides and deletes the test job. UI fixtures cover
manual execution status and opening the run. Automatic scheduling while sushiAI is open is now available through the
explicit control described below. Managing an independent gateway for operation
while sushiAI is closed remains unfinished.

Automatic scheduling is opt-in and global to local Hermes profiles, matching the
native Desktop ticker. The Schedules panel shows gateway liveness separately and
can enable/disable sushiAI's owned executor. The preference survives restart;
quitting sushiAI stops its executor. Interactive profile backends remain separate.
The implementation uses native `HERMES_DESKTOP=1` scheduling, including Hermes's
own store claims and profile gateway checks, rather than a JavaScript cron loop.

Native integration verification created a due one-shot job and observed a
completed run without invoking the manual trigger. It then disabled and closed
the owned executor. Unit tests cover opt-in behavior, persisted preferences,
idempotent start and shutdown before startup; UI fixtures cover the toggle.

### Model settings

Native model-selection responses with `ok: false, confirm_required: true` now
produce an explicit confirmation step instead of an error. The initial request
never auto-confirms. The model panel also reads and writes the profile's default
reasoning effort through native `config.get` / `config.set`; these defaults apply
to new conversations. The isolated `scripts/hermes-settings-smoke.cjs` verifies
read, disable, high-effort selection and on-disk persistence without contacting a
model provider. Unit and Electron fixtures verify model confirmation routing and
the reasoning control. Provider authentication/setup and per-conversation model
settings remain part of the unfinished broader integration.

### Conversation settings

The composer exposes session-specific model and reasoning controls through a
provider capability and a separate Hermes settings module. Model changes keep
the current provider by default; an explicit provider ID is optional. Native
expensive-model confirmation remains a separate user action. Native warnings
are displayed without misreporting a successfully applied setting as a failure.
Changes are disabled during a turn, use runtime IDs, and do not write profile
defaults. Pending settings operations also prevent concurrent sends/deletion.

`node scripts/hermes-conversation-settings-smoke.cjs` runs installed Hermes with
a temporary profile and local model endpoint. It verifies effective reasoning,
executes a prompt with the newly selected model, preserves a second conversation
and profile defaults, and exercises a successful switch with a native warning.
The Electron smoke covers both controls and the confirmation flow. Conversation
settings also suggest models from configured Hermes providers, filtered by the
current or explicitly selected provider. Named `custom:...` connections are
supported. Suggestions load independently; manual model entry remains available
if catalog discovery fails. Only provider names and model IDs cross into React.
Native smoke coverage checks the local endpoint's actual model listing, and
Electron coverage checks provider-dependent suggestions.

Ordinary chats now use native eager runtime restoration rather than the child-watch
resume mode, so their stored model and reasoning are loaded before settings are
reported. Opening a cold chat can therefore wait for Hermes agent initialization.
New ordinary chats do not carry the Bot Chat profile-following flag. The primary
Bot Chat retains Hermes's deliberate profile-following behavior; earlier local
development chats created with that flag are not migrated automatically.

The native settings smoke also verifies persisted reasoning metadata before and
after shutdown, reopens both conversations through a new provider/backend, and
executes a second request with the restored model. Session reasoning is preserved
through model changes using native setters, since a native model switch re-resolves
reasoning from profile defaults. History-loading progress is kept outside the
message transcript; failures remain visible. sushiAI recovery notices survive
subsequent history refreshes. The settings smoke also verifies a crash with native
automatic continuation enabled: Hermes continues exactly once, and the UI state
and recovery notice identify it. Settings in never-initialized empty drafts and
migration of older profile-following development chats need further coverage.

### Recovery policy

The Recovery capability add-on exposes native automatic continuation for the
selected agent. Its checkbox changes only `desktop.auto_continue.enabled` through
the native configuration endpoint, preserving freshness and attempt limits.
The UI identifies the profile-wide scope, including other Hermes interfaces,
and explains that changing the policy does not interrupt a running task.

The native settings smoke verifies enabled/disabled round trips in a disposable
profile; Electron exercises the checkbox and saved state. Protocol regression
coverage checks profile isolation, native default coercion and a minimal config
patch with no credential values returned to the renderer.

Build verification note: the local esbuild config-bundling subprocess intermittently
blocked in a macOS file-open call. The renderer was verified with
`node node_modules/vite/bin/vite.js build --configLoader native` and a separate
`node node_modules/typescript/bin/tsc --noEmit` check. The project build script
has not been changed for this machine-specific issue.

## Attachments — local integration

The provider-neutral `attachments` capability enables a per-conversation file
picker. Draft files stay in renderer memory across mode switches and are not
written to tab metadata. Send accepts up to eight files, each between one byte
and 1 MB; this keeps each upload below the current transport frame limit.
The main process validates names and canonical base64 before calling Hermes.
Images use native `image.attach_bytes`; other files use `file.attach` and its
returned `@file:` reference. No renderer-supplied filesystem paths are accepted.
A failed batch detaches images whose upload was acknowledged. An unconfirmed
upload blocks further sends in that conversation until sushiAI restarts, because
a timeout cannot prove whether Hermes queued an image. This conservative recovery
flow still needs refinement before claiming full attachment parity.

`hermes-attachments-smoke.cjs` uses a disposable Hermes home and a local model
endpoint. It verifies file bytes on disk, a file reference delivered to the model,
and native image staging/detachment. It also sends an image, loads its native media response, restarts Hermes and
verifies the same image from persisted conversation history. It does not yet
verify visual understanding or PDF page rendering. Electron smoke coverage verifies selection, removal and
submission of synthetic attachment bytes. Large-file transfer and PDF-specific workflows remain open. Clipboard file/image paste and file drop are implemented. Plain
text paste retains the native editor behavior.

Attachment draft reads now use a provider-neutral renderer store with immutable
snapshots. A read is bound to its originating conversation identity, survives
Agent view unmount/remount, and blocks that draft's send until it completes.
Electron smoke coverage includes delayed FileReader completion across tab and
Code/Agent mode changes, file drop, synthetic image paste, oversized-file and
nine-file rejection. These tests never read the system clipboard or user files.

## Conversation image previews

The `image-preview` capability uses `conversations.media` with an existing
conversation/item ID. The Hermes module resolves the path from that conversation
and calls native `/api/media`; no arbitrary renderer path or URL is accepted.
Native media-root and symlink restrictions remain authoritative. The response
must be a bounded canonical raster data URL (PNG, JPEG, GIF, WebP or BMP).
SVG/HTML and remote URLs are not embedded by this operation. The transport's
2 MB JSON response limit still applies to image previews.

Persisted user `@image:` directives become image items in transcript order,
including quoted paths. Live uploads expose the same kind of item. Images load
near the visible transcript and report missing/unsupported data with a retry
button. Electron smoke coverage checks a failed request followed by successful
image decoding; native smoke verifies preview bytes before and after a cold
Hermes restart. General assistant Markdown images, standalone structured image
blocks without a saved path, SVG previews and full-size/export controls remain
incomplete and must not be described as full media parity.

## Agent file module

The Files add-on uses native Hermes `/api/fs/default-cwd`, `list`, `read-text`
and `write-text` routes, scoped to the agent's dedicated backend. It supports
folder navigation, text preview and editing. Binary or truncated previews cannot
be saved. Text writes are limited to 512 KB. Drafts stay in renderer memory across
panel/mode switches, separated by provider and agent; they are not stored on disk.

A save re-reads the file and compares a content/path/profile revision, serializes
sushiAI saves for that agent, invokes the native atomic replacement, then verifies
the resulting text. Hermes does not offer an atomic compare-and-swap endpoint:
an independent writer can still race between the preflight read and replacement.
Do not claim external-writer locking. Native smoke covers file browsing, a real
save, and preservation of an external edit after stale-save rejection, exclusively
inside disposable data. Electron covers draft preservation and save feedback.
Creating, renaming, moving and deleting files, full Git management, binary/media file
previews and HTML browser integration are still required for full file parity.

## Git review module

The Git add-on resolves the native project Git root, reads `/api/git/status` and
`/api/git/review/list`, and displays `/api/git/file-diff` results against HEAD.
It includes untracked files via Hermes's native diff implementation. All requests
are GETs to the selected agent's backend; file paths must be repository-relative
and cannot escape through parent segments. The UI shows branch, ahead/behind,
changed files and line-colored diff text. A non-repository is distinct from a
clean working tree. List display is capped at 2,000 entries and response sizes
remain bounded by the transport.

Native verification creates a disposable Git repository with a synthetic fixture
commit, modifies one tracked file and adds one untracked file, then verifies both
diffs through Hermes. It never commits or pushes sushiAI itself. Electron smoke
checks selecting a changed file and rendering its diff. Separate staged/unstaged
views, branch comparisons, unborn-HEAD handling, Git mutations and PR workflows
remain open for full Hermes parity.

Text previews containing U+FFFD are compared against native `read-data-url`
bytes before editing is allowed. A lossy UTF-8 decode or failed byte verification
makes the preview read-only, including a main-process save guard. A literal,
valid UTF-8 replacement character remains editable. Native tests verify original
invalid bytes are preserved and valid Unicode can be saved; Electron checks the
read-only notice and disabled save control. This does not yet provide transcoding
or editing support for legacy encodings.

The Files add-on now previews PNG, JPEG, GIF, WebP and BMP through native
`/api/fs/read-data-url`. These responses reuse raster data-URL validation and the
transport size limit. Raster previews do not expose the text editor; the main
process also rejects text saves for them. Native smoke verifies the returned
image bytes, and Electron verifies actual decoding and absence of text controls.
SVG, PDF, audio/video and HTML file previews remain incomplete.

## Skill command invocation

The `skill-commands` capability exposes a searchable picker using native
`commands.catalog`. Invocation revalidates the selected command against that
profile's skill entries and calls `command.dispatch`, then sends its native
expanded message. Display text for the live user message is kept separate from
model-facing skill content. The synthetic native test creates a disposable skill,
verifies discovery and checks that the model receives both skill content and user
arguments. Electron verifies picker search and invocation arguments. Built-in
slash commands, quick/plugin command routing, bundles, inline slash completion
and durable history projection of skill scaffolding remain incomplete.
