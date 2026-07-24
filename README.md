# Crane

<p align="center">
<img src="screenshot/crane-hero.png" width="49%" alt="Crane — project tree and terminal">
<img src="screenshot/crane-multi-pane.png" width="49%" alt="Crane — multi-pane layout with file editor">
</p>

Native, GPU-rendered desktop development environment for orchestrating terminals, file browsing, diffs, and git workflows across isolated git workspaces.

Built in pure Rust on [warpui](https://github.com/warpdotdev/warp) — Warp's MIT-licensed, wgpu-backed GPU UI framework, vendored under `vendor/warp` — with Warp's `warp_editor` for the file pane and an in-house terminal core (`crates/crane_term`) wrapping [vte](https://github.com/alacritty/vte) for VT parsing and [portable-pty](https://github.com/wez/wezterm/tree/main/pty) for cross-platform PTY.

---

## Download

Grab the latest build for your platform from the [Releases](https://github.com/rajpootathar/Crane/releases) page.

- **macOS** (Apple Silicon + Intel, universal) — `Crane-<version>-universal.dmg`
  - Double-click to mount, drag **Crane.app** into `/Applications`.
  - First launch: right-click the app → **Open** (ad-hoc signed, macOS asks once).
  - If macOS says **"Crane is damaged and can't be opened"**, strip the download-quarantine bit:
    ```bash
    xattr -dr com.apple.quarantine /Applications/Crane.app
    ```
    Then open normally. (This happens on unsigned/unnotarized builds; a paid Apple Developer ID would fix it at the source.)
- **Linux** (x86_64, Debian/Ubuntu) — `crane_<version>_amd64.deb`
  - `sudo dpkg -i crane_<version>_amd64.deb`
- **Windows** (x86_64) — `Crane-<version>-windows-x86_64.zip`
  - Extract, run `crane.exe`.

---

## Features

### Workspaces & projects
- **Project → Workspace → Tab → Layout → Pane** hierarchy. Each Workspace is a git worktree (`git worktree add`) so branches are real filesystem checkouts, not virtual switches.
- **Drag-drop in the Left Panel (Projects tree)** to reorganize freely:
  - Reorder **projects** up/down the list.
  - Reorder **workspaces** within a project, or move them between projects of the same git remote.
  - Reorder **tabs** within a workspace, or drag them into another workspace.
  - Drop is scoped per "group block" so a nested-repo group's children can't escape the group accidentally; folder headers themselves are draggable.
- **Loose-files projects** — folders that aren't a git repo surface their contents as a flat tree; nested `.git` roots auto-promote to sub-projects under a group header.
- **Session restore** — projects, workspaces, tabs, layout splits, open files, panel widths, fonts, and terminal snapshots all persist to `~/.crane/warpui-state.json` and reload exactly as left.
- **Workspace lifecycle** — Cmd+Q confirmation, ghost-worktree pruning when the dir disappears outside Crane.

### Panes
- **Split panes** with draggable dividers; horizontal / vertical splits via `Cmd+D` / `Cmd+Shift+D`. Focus border highlights the active pane.
- **Pane types:**
  - **Terminal** — in-house VT parser (`crates/crane_term`) on top of `portable-pty`. Owns its own grid, scrollback, `?2026` synchronized-output replay, resize-aware reflow, wrap-aware copy, reverse-wraparound for `\b` / `CSI D`. 38 unit tests pin the VT behaviour. **Known issues:** hyperlink (OSC 8) rendering is incomplete; assorted micro-issues with edge-case escape sequences. Custom zsh prompts that compute cursor-back against UTF-8 byte width (Forge theme, older Powerlevel10k) misposition the cursor — this is the shell's bug, not Crane's, but it shows up here.
  - **Files** — tabbed editor built on Warp's `warp_editor` with `syntect` highlighting, find / replace / goto-line, read-only mode for files outside any workspace, native trash on delete, and per-workspace file-tab persistence.
  - **Diff** — unified diff with hunk-level stage button, per-hunk jump nav, syntax highlighting on both sides. Image files short-circuit to an image block instead of a text diff.
  - **Markdown** — `pulldown-cmark` render with soft-wrapping composite paragraphs, tables, nested lists, links, inline images, strikethrough, and task lists. Accent-tinted headings, code-fence styling.
  - **Image** — raster + vector viewer (`png jpg jpeg gif bmp webp ico svg`) with fit-to-window, zoom, and animated-GIF playback, built on warpui's `Image` element.
  - **Browser** *(alpha)* — `wry`-backed embedded webview (macOS / Linux / Windows). **Known issues:** Cmd+A / Cmd+C / Cmd+V and similar editing shortcuts don't reliably reach the webview's input fields; autocomplete / form-autofill is unreliable. Use sparingly until shortcut forwarding lands.

> **Planned:** a **PDF pane** (`pdfium-render`, page nav + zoom + text selection) is specced and scheduled but not yet shipped — see `docs/superpowers/plans/2026-07-19-pdf-viewer.md`. A markdown **edit ⇄ preview toggle** is in progress.

### Terminal command history
- **OSC 633 shell integration** — a zsh integration script is installed at startup so Crane records each command, its cwd, and exit status.
- **Session-ranked history** — Up / Down at an empty prompt ranks recent commands against the shell's live working directory, so the most relevant command for *where you are* surfaces first. Capped at 5000 entries in memory and on disk.

#### LSP *(alpha)*

> 🚧 **Servers run for the whole app session.** Per-language stdio multiplexer (`src/lsp/`). Completion / hover / goto-definition (`F12`) work but quality is uneven across servers; complex completions (snippets, signature help) are incomplete. **Known issue:** a started server stays alive until you quit Crane (no idle shutdown yet), which can grow RAM / fan-noise on long sessions with many languages opened.

### Git
- **Right Panel Changes** — staged / unstaged split, stage/unstage by file or hunk, commit (Cmd+Enter), push, pull, fetch, across every repo in the active workspace.
- **Branch picker** — `Cmd+Shift+B` opens the Switch Branch modal to switch / pick branches across all repos in the active workspace.
- **External edits land instantly** — any file touched outside Crane (other editor, build script, sub-agent in a terminal pane) reflects in the Changes tab within ~50 ms via the file watcher.

#### Git Log Pane — `Cmd+9` (alpha)

> 🚧 **Alpha — may break, regress, or render incorrectly.** The DAG layout and `.git/refs/` watcher are new code paths; expect rough edges on edge cases (octopus merges, very deep histories, rebases-in-flight).

- DAG graph with lane-based layout, branch-stable lane colors, merge-into-existing-lane termination so curves connect back to the branch origin.
- Ref pills inline on each row: green = HEAD, purple = local branch, blue = remote, yellow = tag.
- Filter by subject / hash / author (typed) + by branch (combo) + by user (combo). Filter signature is cached so typing doesn't recompute lanes per keystroke.
- Right-click commit row → Checkout · Create branch from here · Create worktree from here · Cherry-pick · Revert · Copy hash.
- Auto-refresh on `.git/HEAD`, `.git/refs/`, `.git/packed-refs` writes (debounced), with a poll backstop that only fires when the watcher has been quiet.

### Performance & architecture
- **No async runtime** — plain `std::thread` + `parking_lot` + `mpsc`. No Tokio. The PTY reader lives on its own thread and wakes the GPU frame loop via warpui's repaint channel.
- **Idle is free** — at rest Crane runs zero git subprocesses and does zero per-frame `read_dir`; git status, diffs, and git-log reloads run off the render thread and are cached, so only real changes wake the system.
- **One file watcher** (`src/warpui/file_watcher.rs`) for the whole app — `notify` with event coalescing and prefix routing to the owning project (macOS FSEvents, Linux inotify, Windows ReadDirectoryChangesW), filtering `.git/objects/`, `.git/logs/`, and editor temp files.
- **Cached diff renders** — `TextDiff::from_lines` + `git diff` computed off-thread and cached; re-rendering an unchanged diff is a cheap `Arc::clone`.
- **Lazy init** — a session that never opens a project pays no background thread cost.

### Fonts, themes, accessibility
- **Bundled fonts + system fallback** — JetBrains Mono + phosphor icon glyphs bundled; CJK / Arabic / Hebrew / Devanagari fall back to system fonts so non-Latin scripts render correctly.
- **Live theme switcher** — built-in dark / light themes plus user themes at `~/.config/crane/themes/*.toml`, loaded and hot-swapped at runtime without a restart.
- **Font size** — `Cmd+=` / `Cmd+-` / `Cmd+0`, persisted across sessions.
- **Confirm-before-quit** — Cmd+Q prompts; prevents accidental dismissal.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Cmd+T` | Split active Pane with a new terminal |
| `Cmd+Shift+T` | New Tab in active Workspace |
| `Cmd+D` / `Cmd+Shift+D` | Split horizontally / vertically |
| `Cmd+W` / `Cmd+Shift+W` | Close focused Pane / close active Tab |
| `Cmd+[` / `Cmd+]` | Focus prev / next Pane |
| `` Cmd+` `` / `Cmd+~` | Tab switcher (forward / backward) |
| `` Ctrl+` `` / `` Ctrl+Shift+` `` | Cycle Left-Panel Projects (next / previous) |
| `Cmd+B` | Toggle Left Panel |
| `Cmd+Shift+B` | Switch Branch modal |
| `Cmd+/` | Toggle line comment (in editor) / toggle Right Panel |
| `Cmd+9` | Toggle Git Log Pane on active Tab |
| `Cmd+O` / `Cmd+Shift+O` | Open external file / add folder as project |
| `Cmd+Shift+U` / `Cmd+Opt+T` | Open Browser pane / new Browser tab |
| `Cmd+Shift+N` | Open Welcome pane beside the focused pane |
| `Cmd+F` / `Cmd+Shift+F` | Find in active editor / Find in Files |
| `Cmd+H` | Find-and-replace in active editor |
| `Cmd+G` | Goto line in active editor |
| `Cmd+Opt+W` | Toggle soft word-wrap in the focused editor |
| `Cmd+=` / `Cmd+-` / `Cmd+0` | Font size up / down / reset |
| `Cmd+S` | Save the active file |
| `Cmd+C` / `Cmd+X` / `Cmd+V` | Copy / cut / paste in the focused pane |
| `Cmd+A` / `Cmd+Z` / `Cmd+Shift+Z` | Select all / undo / redo |
| `Cmd+K` | Clear the focused terminal |
| `F12` | LSP goto-definition at the caret |
| `Cmd+Enter` | Submit commit (when the commit message field is focused) |
| `Cmd+Q` | Quit (with confirmation modal) |

---

## Build from source

Requires **Rust (edition 2024 toolchain)** and platform-specific system dependencies. The Warp UI framework is vendored as a git submodule, so clone recursively.

```bash
git clone --recurse-submodules https://github.com/rajpootathar/Crane.git
cd Crane
# if you already cloned without submodules:
git submodule update --init --recursive
cargo run --release
```

### macOS

- No extra system deps. macOS 11+ recommended.

### Linux

```bash
sudo apt install \
  libxkbcommon-dev libwayland-dev libgl-dev libx11-dev \
  libxcb1-dev libxrandr-dev libxi-dev libxcursor-dev pkg-config
```

### Windows

Needs the MSVC toolchain (via Visual Studio Build Tools). No other prerequisites.

---

## Packaging

The `Makefile` wraps `cargo-bundle` + `hdiutil`. Run from the repo root:

```bash
make help                # list targets
make bundle              # build .app for the host arch
make dmg                 # bundle + .dmg
make release             # == dmg
make bundle-universal    # arm64 + x86_64 → universal .app
make dmg-universal       # universal .app → .dmg
make release-universal   # == dmg-universal
make upload TAG=v0.1.0   # create a GitHub release and attach the DMG
make clean               # remove bundles / DMGs
```

One-shot release helpers bump the version, tag, push, and upload in a single step:

```bash
make ship                # patch bump → release → tag → push → upload
make ship-minor          # minor bump
make ship-major          # major bump
make ship-universal      # patch bump, universal DMG
```

Output paths:
- `target/release/bundle/osx/Crane.app`
- `target/release/Crane-<version>-<arch>.dmg`
- `target/release/Crane-<version>-universal.dmg`

`make icns` regenerates `icons/crane.icns` from `crane.png` using `sips` + `iconutil`.

## Automated releases

Pushing a tag `vX.Y.Z` triggers [`.github/workflows/release.yml`](.github/workflows/release.yml), which builds on macOS / Linux / Windows runners and attaches:

- `Crane-<version>-universal.dmg`
- `crane_<version>_amd64.deb`
- `Crane-<version>-windows-x86_64.zip`

…to the GitHub Release for that tag.

```bash
git tag v0.1.0
git push origin v0.1.0
```

---

## Architecture

Single binary. Pure Rust. No Electron, no web runtime, no async runtime. `main.rs` hands off directly to `warpui::run()` — warpui (`src/warpui/`) is the sole front-end.

```
src/
├── main.rs          entry point → warpui::run()
├── startup.rs       process init before the UI spawns a PTY
├── warpui/          the entire GPU-rendered front-end (warpui + warp_editor)
│   ├── shell.rs         top-level shell: panes, tabs, layout, shortcuts, persistence
│   ├── controller.rs    app state + action dispatch
│   ├── layout.rs · split.rs         Layout tree (Leaf / Split) + draggable splitters
│   ├── file_pane.rs · editor_view.rs · file_tree.rs   Files pane (warp_editor + syntect)
│   ├── diff_view.rs     Diff pane (hunk staging, image blocks)
│   ├── markdown_view.rs Markdown pane (pulldown-cmark)
│   ├── image_view.rs    Image pane (warpui Image element)
│   ├── browser_view.rs · browser.rs   wry webview pane
│   ├── git.rs · git_log.rs · git_log_element.rs   git status + DAG log
│   ├── history_store.rs · shell_init.rs   OSC 633 shell integration + ranked history
│   ├── file_watcher.rs  one notify watcher, prefix-routed to projects
│   ├── persist.rs       ~/.crane/warpui-state.json save / restore
│   └── theme.rs · bundled_fonts.rs · icons.rs   theming, fonts, phosphor icons
├── (crates/crane_term)  in-house VT parser + grid + scrollback + reflow (38 tests)
├── git.rs           shell-out git core (status · stage · commit · push · worktree)
├── lsp/             LSP client (per-language stdio multiplexer)
├── format/          external formatter integration (prettier, etc.)
├── theme.rs         theme model + ~/.config/crane/themes/*.toml loader
└── syntax.rs        syntect setup
```

Warp's framework crates (`warpui`, `warp_editor`, `string-offset`) are consumed as path dependencies from the `vendor/warp` submodule, kept out of Crane's own workspace so cargo treats them as external.

See [CLAUDE.md](CLAUDE.md) for project conventions + the canonical naming glossary (Left Panel / Main Panel / Right Panel; Project → Workspace → Tab → Layout → Pane).

## Known issues

- **Cursor drifts a few columns short of `%` with certain custom zsh prompts.**
  Some prompt frameworks (observed with the Forge theme; also older
  Powerlevel10k versions) compute their RPROMPT cursor-back escape
  against UTF-8 byte width instead of column width for Nerd-Font / PUA
  icons. Each 3-byte icon over-counts by 2 cells, so the cursor lands
  `2 × icon_count` columns short of the prompt end. Crane's VT grid is
  correct — the shell is writing the wrong `\e[<n>D`. Workaround:
  disable the offending theme or switch its icon set to ASCII.
- **Older scrollback rows imperfectly reflow on repeated width changes.**
  Rows that pre-date the wrap-on-actual-wrap logic may not perfectly
  unwrap when the terminal is widened back after several resizes.
  Recent content reflows correctly; a fresh shell `clear` recovers.

## Tests

```bash
make test            # or: cargo test --bin crane
```

Covers, among others:
- Pure Layout tree operations (split, remove, first-leaf, collect-leaves, contains, set-ratio).
- Session persistence: `warpui-state.json` round-trips and backward-compatible deserialization of older state files.
- File watcher: filter list, prefix-route to project, end-to-end create+modify via tempdir.
- Terminal command history: OSC 633 decode, session-ranked ordering against cwd.
- Markdown: tables round-trip without leaking cell text into adjacent paragraphs; nested lists close without a phantom bullet; composite paragraphs wrap.
- crane_term: VT parser, `?2026` sync replay, scrollback, reflow on resize (38 tests).

---

## License

[MIT](LICENSE) © rajpootathar
