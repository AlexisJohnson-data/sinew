# Sinew — Personal fork

> **TL;DR** — This fork adds a WSL/Ubuntu shell mode, a one-click
> Windows→WSL project migration agent, **real in-agent browser automation
> (CDP/Chromium)**, **PDF reading (text + scanned pages via the model's
> vision)**, **file-to-chat context** (drag, right-click, clipboard paste),
> **quote-a-reply-into-context**, a friendlier chat UX (iMessage-style
> bubbles, code-block copy, Ctrl+F search), workflow notifications,
> Quick Open (Ctrl+P), and a handful of layout tweaks that make Sinew
> usable when you actually live on Windows but code in Linux.

Built on top of [Paseru/sinew](https://github.com/Paseru/sinew) — every
feature here is additive. Pull upstream whenever; the fork stays on a
single `feat/wsl-shell` branch so merges stay clean.

If you just want the **Windows installer**, grab the `.msi` from
[Releases](../../releases) and skip to *Configure a model provider*.

If you want to build / further customize it, read on.

---

## Why this fork exists

Plain Sinew is great if you write code natively on the OS Sinew runs
on. But many of us (me included) run Windows for everything **except**
dev, and live inside WSL2 (`\\wsl$\Ubuntu\home\me\projects\…`). The
agent's `bash` tool needs to execute commands where the project
actually is — running `npm install` from PowerShell on a project under
`\\wsl$` is a recipe for cursed `node_modules`.

So:

1. The agent now knows how to spawn `wsl.exe -- bash -lc` for any
   command, with a workspace-aware `Auto` mode that picks PowerShell
   or WSL **based on the path of the open workspace**.
2. A built-in **Migration agent** copies a Windows-hosted project to
   WSL safely (git-clone if the tree is clean, file-copy with
   exclusions if dirty), surfaces env risks, and gives you a
   post-migration checklist.

Everything else in this list is quality-of-life that fell out of
daily use.

---

## Feature index

**Agent capabilities**
- [Real browser automation (CDP / Chromium)](#real-browser-automation-cdp--chromium)
- [PDF reading: digital text + scanned pages via vision](#pdf-reading-digital-text--scanned-pages-via-vision)
- [File → chat context: drag, right-click, clipboard paste](#file--chat-context-drag-right-click-clipboard-paste)
- [Quote a reply into the composer](#quote-a-reply-into-the-composer)
- [Gemini / Antigravity reliability fix](#gemini--antigravity-reliability-fix)

**Workspace & UX**
- [WSL / Ubuntu shell support](#wsl--ubuntu-shell-support)
- [Git panel routes through WSL on WSL workspaces](#git-panel-routes-through-wsl-on-wsl-workspaces)
- [Smart Auto shell preference](#smart-auto-shell-preference)
- [Windows → WSL migration agent](#windows--wsl-migration-agent)
- [MCP import from Claude Code / Codex](#mcp-import-from-claude-code--codex)
- [Migration agent model picker](#migration-agent-model-picker)
- [Quick Open (Ctrl+P) and Chat search (Ctrl+F)](#quick-open-ctrlp-and-chat-search-ctrlf)
- [Workflow notifications](#workflow-notifications)
- [Layout: collapsible panels, separate Settings/Remote windows](#layout-collapsible-panels-separate-settingsremote-windows)
- [Chat UX: iMessage bubbles + code-block copy](#chat-ux-imessage-bubbles--code-block-copy)
- [Skills: thematic grouping, accordion, "Enable/Disable all"](#skills-thematic-grouping-accordion-enabledisable-all)
- [Terminal: smart Ctrl+C copy / Ctrl+V paste](#terminal-smart-ctrlc-copy--ctrlv-paste)
- [Welcome screen: shell picker, Settings entry, migration entry](#welcome-screen-shell-picker-settings-entry-migration-entry)
- [Recents: pin & remove](#recents-pin--remove)
- [Welcome "What's new" capabilities card](#welcome-whats-new-capabilities-card)
- [Open maximized + centered on Windows](#open-maximized--centered-on-windows)

---

### Real browser automation (CDP / Chromium)

The agent can drive a real Chromium browser over the Chrome DevTools
Protocol — open pages, screenshot, read the DOM, click, type, eval JS,
etc. — so it can actually *look at* a running site instead of guessing
from source. Cherry-picked and adapted from Glamgar's work on
[Paseru/sinew#28](https://github.com/Paseru/sinew/pull/28).

- New crate `crates/sinew-browser/` (chromiumoxide 0.7 + tokio): session
  management, DOM, screenshots, gif recording.
- 18 agent tools in `crates/sinew-app/src/browser.rs`
  (`browser_open`, `browser_screenshot`, `browser_dom`, `browser_click`,
  `browser_eval`, …), names in `tool_names.rs` (`BROWSER_*` consts).
- Wired into the turn in `crates/sinew-app/src/agent/turn.rs`: the
  browser descriptors are added **only when `browser_enabled` and not in
  Plan mode**, plus a `<browser_tools>` system-prompt block so the agent
  knows the browser exists (otherwise it tries to `apt-get install
  chromium` in the shell).
- **Chrome is preferred over Edge** in `session.rs::find_browser_executable`
  (searches per-user `%LOCALAPPDATA%` paths too).
- **Headed by default**: chromiumoxide's builder injects `--headless`,
  so we call `.with_head()` explicitly when `headless = false` — you
  actually *see* Chrome navigate.
- **Force-browser toggle** (globe button in the composer): one-shot mode
  that prepends an instruction making the agent use the `browser_*` tools
  instead of reading code. `src/components/chat/ChatPane.tsx`.
- **Inline screenshots**: `browser_*` tool cards render the returned
  screenshot directly under the card (`src/components/chat/ToolCard.tsx`).

Needs Chrome or Edge installed on the host (not bundled).

### PDF reading: digital text + scanned pages via vision

The `read` tool now reads PDFs. A router picks the right path per
document, so the common case stays free/local and only scans pay for
heavier processing:

- **Digital PDF** (has a text layer, ~90% of PDFs) → `liteparse`
  extracts the text as **markdown**, read by line ranges like any text
  file. Free, fully local, ~3 ms/page, exact.
- **Scanned PDF** (empty/sparse text layer, heuristic <50 chars/page) →
  `liteparse` renders the pages to PNG via PDFium and **attaches them as
  images**, so the agent's own vision model (Gemini / Claude / GPT) reads
  them. No OCR engine, no setup — the LLM is the OCR. Capped at 10 pages.

Notes:
- `liteparse` is added with `default-features = false` to **drop the
  `tesseract` feature** — built-in Tesseract needs CMake to build
  leptonica/tesseract from C++ source (fails on a clean Windows box) and
  we don't want it anyway. The tool result tells the agent *not* to shell
  out to an external OCR.
- A higher-accuracy **Mistral OCR** backend (optional API key) is the
  planned next layer — purely additive.
- **Packaging caveat**: PDFium ships as a `pdfium.dll` (~6.7 MB) loaded
  at runtime; the MSI must bundle it next to the exe
  (`liteparse-pdfium` looks in the `current_exe` dir as a fallback) or
  PDF support breaks on machines other than the build host.

Code: `crates/sinew-app/src/read.rs` (`read_pdf`, scan detection,
`render_pdf_pages`), dep in `crates/sinew-app/Cargo.toml`. PDF reads
render as a normal expandable tool card so you can see the extracted
markdown / page thumbnails (`src/components/chat/ToolCard.tsx`).

### File → chat context: drag, right-click, clipboard paste

Get files into the agent's context without typing paths:

- **Drag** an entry from the file tree onto the chat / composer →
  added as context.
- **Right-click → "Add to chat"** on a file.
- **Paste** into the file tree (`Ctrl+V` on a folder): an Explorer-copied
  file or a clipboard **screenshot** is imported into the workspace.

Cross-component handoff uses a module singleton + a window
`sinew:add-files-to-chat` CustomEvent. Windows clipboard files are read
via PowerShell `Get-Clipboard -Format FileDropList`; screenshots via
`navigator.clipboard.read()` image blobs (the keyboard `Ctrl+V` handler
`preventDefault`s the native paste, so the blob fallback is required).

Code: `src/components/FileTree.tsx` (drag, context menu, `onPaste` /
`importClipboardImages`), `src/components/chat/ChatPane.tsx` (event
listener), `src-tauri/src/platform.rs` (Windows `FileDropList`).

### Quote a reply into the composer

Select part of an assistant message → a small **"Add to context"** pill
appears → click it to prepend the selection as a `>` blockquote in the
composer, so your next turn can respond to a specific fragment.

Guarded against the trailing synthetic `click` that a small drag-select
fires (the pill ignores clicks within 250 ms of appearing). Selecting
text inside your **own** message no longer triggers a rewind either
(the rewind handler bails when there's an active selection).

Code: `src/components/chat/ChatPane.tsx` (`quoteAction`,
`applyQuoteToComposer`), `.quote-pill` in `src/styles.css`.

### Gemini / Antigravity reliability fix

Google models (Antigravity endpoint) used to 400 with
`INVALID_ARGUMENT` on any project that already had tool history — fresh
projects worked, started ones didn't. Two root causes, both in
`crates/sinew-google/src/client.rs`:

- Every `functionResponse` was named `generic_tool` (the tool name
  couldn't be recovered from the call id) while the `functionCall` had
  the real name → Gemini matches the two **by name**, so every pair
  mismatched. Fixed by mapping `tool_call_id → name` from the
  `ToolCall` parts and labelling responses correctly.
- Replayed `thought` parts without their original `thoughtSignature`
  were rejected; unsigned thoughts are now dropped instead of sent bare.

A request-body dump to `%TEMP%\sinew-google-bad-request.json` on any
400 was added to make future diagnosis trivial.

### WSL / Ubuntu shell support

The agent's `bash` tool and the interactive terminal can now run
through `wsl.exe`. Choose your shell in **Settings → Tools → Terminal
& shell**: `Auto`, `PowerShell`, or `WSL`.

Under the hood:

- `crates/sinew-app/src/bash.rs` — new `ShellKind::Wsl` variant,
  `path_targets_wsl_filesystem()`, atomic caches for active shell
  kind and global preference.
- `src-tauri/src/terminal.rs` — `default_terminal_command()` picks
  the correct shell; for WSL we strip the verbatim `\\?\UNC\` prefix
  off the workspace cwd before passing it to `wsl.exe` (it cannot
  translate verbatim paths).
- Default WSL distribution + login shell, so your NVM / asdf / aliases
  load like in a normal `wsl` session.

### Git panel routes through WSL on WSL workspaces

Vanilla Sinew shells out to Windows-native `git.exe` for the Git
panel. On `\\wsl$\…` workspaces that trips the CVE-2022-24765
"dubious ownership" check on every call (Linux UID ≠ Windows user),
so the panel shows **"Not a Git repository"** for a perfectly valid
repo. Every user would have to manually run:

```powershell
git config --global --add safe.directory '%(prefix)///wsl$/Ubuntu/home/<you>/projects/<repo>'
```

…before the panel works.

This fork sidesteps the whole problem: at the `run_checked` /
`run_output` boundary in `src-tauri/src/git.rs`, any git invocation
on a workspace that `sinew_app::path_targets_wsl_filesystem` flags
gets routed through `wsl.exe -- git <args>` with the workspace path
as the working directory. `wsl.exe` translates the cwd to the native
Linux path automatically, git runs inside Linux, sees a UID-matching
repo, and works normally — no user config, no security relaxation.

`C:\` / `D:\` workspaces still use native `git.exe` — only WSL paths
are routed through `wsl.exe`. Mirrors the smart-shell pattern.

### Smart Auto shell preference

`Auto` is the default for fresh installs. When it's active:

- Workspace path starts with `\\wsl$\…` / `\\wsl.localhost\…` →
  **WSL**.
- Anything else (`C:\`, `D:\`, …) → **PowerShell**.
- macOS / Linux → unchanged, always bash.

You no longer have to toggle the global shell when switching between a
Windows-native project and a WSL one. The interactive terminal and the
agent's `bash` tool resolve `Auto` consistently — same shell, same
working directory.

The preference can be flipped from two places: the inline picker on
the Welcome screen (right rail) or **Settings → Tools → Terminal &
shell**. Both surfaces expose the same three explicit options
(`Auto` / `PowerShell` / `WSL`) so there's no ambiguity about what
each one means.

Code: `terminal.rs::resolve_shell_preference()`,
`bash.rs::set_active_shell_for_workspace()`,
`workspace.rs::open_workspace()` (hook on every workspace open).

### Windows → WSL migration agent

Two entry points:

- **Welcome screen**: a secondary CTA *"Migrate a Windows project to
  WSL"*.
- **Sidebar**: a tiny icon next to the workspace name, visible only
  when the open workspace is a Windows path.

The flow:

1. Pick the source folder (Windows path) — file picker or pre-filled
   from sidebar.
2. Pick the target (defaults to `\\wsl$\Ubuntu\home\<you>\projects\<basename>`,
   editable).
3. Sinew creates the target dir if needed, refuses to clobber a
   non-empty target without explicit acknowledgement, and opens it as
   a workspace.
4. A fresh conversation starts in Goal mode, pre-filled with a
   structured migration prompt. You just press Send.

The agent then:

- inspects the source (stack detection),
- checks git state, **uses the `question` tool** to decide between
  re-clone (clean + has remote) and file copy (dirty or no remote),
- copies via `rsync -a` with sensible exclusions
  (`node_modules`, `target`, `dist`, `.venv`, …),
- surfaces env-vars / Windows-isms / `.venv` Python path issues,
- ends with a markdown checklist of post-migration steps.

Code: `src-tauri/src/workspace.rs::prepare_migration_target`,
`crates/sinew-app/src/store.rs::MIGRATION_AGENT_PROMPT`,
`src/components/MigrationDialog.tsx`,
`src/lib/pendingMigration.ts` (cross-component handoff).

### MCP import from Claude Code / Codex

The MCP tab used to start empty for every new install. If you already
ran Claude Code or Codex CLI, you had a perfectly good list of MCP
servers configured there and had to retype each one to use them
inside Sinew.

An **Import…** button in **Settings → MCP servers** now opens a file
picker on `.json` / `.toml`, auto-detects the format from the
extension, and merges the servers into Sinew's settings (duplicates
by name are skipped, so re-running it is harmless):

- **Claude Code / Claude Desktop** — pick `~/.claude.json` (Windows:
  `C:\Users\<you>\.claude.json`; WSL: `\\wsl$\Ubuntu\home\<you>\.claude.json`).
  Reads the top-level `mcpServers` object.
- **Codex CLI** — pick `~/.codex/config.toml`. Reads
  `[mcp_servers.<name>]` sections (older `[mcp.<name>]` also accepted).

The MCP probe re-runs automatically after import so newly-added
servers light up green/red on the spot.

Code: `crates/sinew-app/src/mcp.rs::{parse_mcp_import_file,
merge_imported_mcp_servers}`, Tauri command
`import_mcp_servers_command` in `src-tauri/src/conversations.rs`,
UI in `src/components/SettingsPane.tsx::McpSection`.

### Migration agent model picker

The MigrationDialog lets you pick which LLM runs the migration before
you launch. The select lists every model whose provider you've
configured; the choice rides through the migration handoff and is
persisted as the new conversation's **Goal-mode model** before you
send.

Code: `src/components/MigrationDialog.tsx` +
`src/components/chat/ChatPane.tsx` (prefill handler uses a ref to
`persistModeSelection` to stay stable while still seeing the latest
callback).

### Quick Open (Ctrl+P) and Chat search (Ctrl+F)

- **Ctrl+P** — fuzzy file palette with `:line` suffix support and
  localStorage-backed recents. Custom dependency-free fuzzy matcher
  (`src/lib/fuzzy.ts`) with substring fast-path,
  segment-start/streak/tail-name bonuses. UI:
  `src/components/QuickOpen.tsx`.
- **Ctrl+F (in chat)** — in-message search using the CSS Custom
  Highlight API (`CSS.highlights`, `::highlight()`). No React DOM
  mutation, no re-render storm. Hook + component:
  `src/components/chat/useChatSearch.ts`,
  `src/components/chat/ChatSearch.tsx`.

### Workflow notifications

When the Sinew window isn't focused, get pinged when the agent:

- finishes a turn (`status` transitions from running → idle),
- asks a question (`question` tool fires),
- finalizes a plan (Plan-mode → `planReady`),
- completes a Goal-mode run (`goalWorkflow.status === "complete"`).

OS-level notification via `tauri-plugin-notification`, plus taskbar
flash via `request_user_attention`. Toggleable in
**Settings → Tools → Notifications**.

Helper: `src/lib/notify.ts` (`notify`, `pingUserAttention`,
`isWindowFocused`, `flashTaskbar` — all best-effort, errors
swallowed). Triggers live in `src/components/chat/ChatPane.tsx`.

### Layout: collapsible panels, separate Settings/Remote windows

- The sidebar (files) and chat pane both have collapse rails — a thin
  vertical bar you can click to fold them in.
- The center editor pane is auto-hidden when no file/terminal is
  open. As soon as you open a file or summon a terminal, it slides
  back in.
- **Settings** and **Remote** open in their own dedicated Tauri
  windows (`?view=settings|remote` URL routing). Previously they
  rendered inline in the center pane and got hidden behind a fat
  chat panel. Now each lives in its own native window with normal OS
  chrome.

Code: `src-tauri/src/platform.rs::create_secondary_window`,
`src/components/Workspace.tsx` (the `editorShellVisible` flag and the
`openSettings`/`openRemote` handlers).

### Chat UX: iMessage bubbles + copy buttons

- User messages right-align with a subtle bubble background; assistant
  messages stay left-aligned and unboxed (no more rail accent —
  upstream had a violet stripe I removed).
- Code blocks inside chat messages get a hover-revealed **Copy**
  button, just like Claude Code itself.
- The **whole assistant message** also gets a hover-revealed copy
  button in the top-right corner — useful for grabbing the markdown
  response without selecting it manually. Component:
  `src/components/chat/MessageCopyButton.tsx`.
- Code blocks are wrapped/highlighted via `rehype-highlight`.

### Skills: thematic grouping, accordion, "Enable/Disable all"

The Skills panel was a flat list. Now:

- Skills are bucketed into thematic categories: **Frontend & Design**
  (merged from upstream's separate Design + Frontend), **AI &
  Agents**, **Backend**, **Security**, **DevOps**, etc.
- Each category is a collapsible accordion (closed by default).
- A single **Enable all / Disable all** toggle per category, plus a
  global toggle at the top.

Rules live in `src/components/SettingsPane.tsx`. Order matters:
specific categories (Design / Frontend) are matched before the AI
catch-all so things like `design-taste-frontend` land in the right
bucket.

### Terminal: smart Ctrl+C copy / Ctrl+V paste

- **Ctrl+C** when there's a selection → copies and clears selection.
  No selection → sends `^C` to the shell as normal.
- **Ctrl+V** → pastes the clipboard into the PTY input stream.

Implemented in the xterm.js shell binding. Avoids the usual "I tried
to copy but I killed the process" Windows-terminal trap.

### Welcome screen: shell picker, Settings entry, migration entry

A first-time user opens Sinew and has to know how to wire it before
they even touch a project. The Welcome page surfaces three things:

- A small **gear** in the top-left corner (away from the close
  button) opens Settings in a separate window. Lets you configure API
  keys / providers before opening any workspace.
- A **Terminal shell picker** (Windows only) in the right rail with
  three explicit options — **Auto** / **PowerShell** / **WSL** — and
  a hint that explains, in plain English, what each mode does
  (including the "WSL on /mnt/c is slow" trap). Defaults to Auto.
- A **Migrate to WSL** CTA in the same rail. Click → MigrationDialog.

The whole page fits on one screen now: title + Open folder + recents
in the main column, the two secondary actions in a sticky right rail
(880px stage on Windows, single 520px column elsewhere).

Code: `src/components/Welcome.tsx` +
`src/lib/ipc.ts::openSecondaryWindow` / `getShellPreference` /
`setShellPreference`. Backend: `get_shell_preference` /
`set_shell_preference` Tauri commands in
`src-tauri/src/conversations.rs` — workspace-independent so the
picker is usable before any folder is open.

### Recents: pin & remove

Hovering a recent workspace on the Welcome page reveals two actions: a
**pin** (📌) that lifts it to the top and keeps it there regardless of
recency, and a **remove** (✕). Pinned entries don't count against the
recents cap, so they stay sticky. Persisted in the same localStorage
list with an added `pinned?: boolean` field.

Code: `src/lib/recents.ts` (`removeRecent`, `toggleRecentPinned`,
`compareRecents`), `src/components/Welcome.tsx`, `src/types.ts`.

### Welcome "What's new" capabilities card

Some of this fork's headline powers (PDF reading, browser automation,
quote-to-context) aren't obvious IDE features, so a dismissible **"What's
new / Things this agent can do"** card on the Welcome page surfaces them
once — on the landing screen, so it never clutters the IDE while you
work. Dismissal is persisted (`localStorage` key
`sinew.whatsNewDismissed.v1`); bump the key suffix to resurface it after
a future update.

Code: `src/components/Welcome.tsx` (`CAPABILITIES`), styles
`.welcome__whatsnew*` in `src/styles.css`.

### Open maximized + centered on Windows

Sinew launches **maximized and centered** by default on Windows
instead of opening at some arbitrary half-off-screen size — fixes the
"window spawns dangling off the edge of the screen" issue.

Code: `src-tauri/src/lib.rs` (window creation hook).

---

## Build / install

### Download the MSI (end users)

Pre-built `.msi` for Windows in the [Releases](../../releases) section.
Run it, launch Sinew, configure a model provider in Settings, done.

### Build from source (forkers)

Prereqs (Windows):

- **Rust** via `rustup` (use `winget install Rustlang.Rustup` then
  `rustup default stable`)
- **Visual Studio Build Tools 2026** with the *Desktop development
  with C++* workload (or VS 2022 — both work)
- **Node.js LTS** + npm (`winget install OpenJS.NodeJS.LTS`)
- For the WSL features to be useful: **WSL2 + an Ubuntu
  distribution** (`wsl --install -d Ubuntu`)

Then:

```powershell
git clone https://github.com/<your-username>/sinew.git
cd sinew
git checkout feat/wsl-shell
npm install
npm run tauri dev          # live-reload dev build
npm run tauri build        # produces target\release\bundle\msi\*.msi
```

The `.msi` is signed only if you set up `tauri.conf.json` signing
identities; the unsigned build still installs fine, Windows just
shows a SmartScreen warning the first time.

### Useful WSL setup for the agent

If you want the agent's `bash` tool to find Node tooling inside WSL,
install Node via `nvm` and make sure it auto-loads in `~/.profile`
(not just `~/.bashrc`) — Sinew shells `wsl.exe -- bash -lc <cmd>`,
which sources `.profile`.

```bash
# inside WSL Ubuntu
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
# nvm install in .bashrc, but echo it into .profile too:
echo '[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"' >> ~/.profile
nvm install --lts
```

---

## Sync with upstream

The fork branch is `feat/wsl-shell` based on `Paseru/main`. To pull
upstream changes:

```bash
git remote add upstream https://github.com/Paseru/sinew.git    # one-time
git fetch upstream
git checkout feat/wsl-shell
git rebase upstream/main
# resolve conflicts (usually in src/components/Workspace.tsx and
# Settings-related files since those are the most-touched here)
```

The Rust crate boundary (`crates/sinew-app/src/bash.rs`,
`store.rs`) and the `src-tauri/` side stay tightly scoped; conflicts
there are rare. Frontend conflicts mostly resolve cleanly by
preferring "ours" for the layout / Welcome / Settings sections and
"theirs" for everything else.

---

## Where to start customizing

If you want to extend this fork further:

| Area | Start here |
|---|---|
| New shell variant | `crates/sinew-app/src/bash.rs::ShellKind`, plus the matching branch in `src-tauri/src/terminal.rs::default_terminal_command` |
| Another secondary window (e.g. "Stats") | `src-tauri/src/platform.rs::create_secondary_window` + URL view param in `src/main.tsx` |
| A new notification trigger | `src/components/chat/ChatPane.tsx` — search for `pingUserAttention` |
| Another Quick-actions palette | clone the `QuickOpen.tsx` pattern (modal + `fuzzy.ts`) |
| Another agent flow (like the migration one) | template lives in `crates/sinew-app/src/store.rs` as a `pub const`, plus a small `pendingX` module singleton + window event for the cross-component handoff |
| A new Welcome CTA | `src/components/Welcome.tsx` — secondary buttons follow the same `welcome__cta--secondary` pattern |

---

## License

Inherits Paseru/sinew's license (see `LICENSE`). This fork claims no
additional rights — it's just my personal customization layer.
