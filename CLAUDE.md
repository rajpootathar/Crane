# Crane — Project Ruleset

Native GPU-rendered desktop development environment built in pure Rust with egui/wgpu.

## Tech stack

- **Language**: Rust edition 2024
- **GUI**: eframe 0.34 + egui 0.34 + wgpu backend
- **Terminal**: in-house `crane_term` crate (`crates/crane_term`) wrapping `vte` 0.15 (parser only) + portable-pty 0.9 (cross-platform PTY). Owns the grid, scrollback, ?2026 sync replay, and resize reflow.
- **Concurrency**: parking_lot mutexes, `std::thread` for PTY reader; no async runtime
- **Git**: shell out to the `git` binary via `std::process::Command` — never `git2`, never `libgit2`
- **Text / markdown / diff**: syntect (syntax highlighting), pulldown-cmark (markdown), similar (diff)
- **File dialogs**: rfd (native folder/file pickers)
- **Icon**: loaded from `crane.png` via `image` crate, set as app icon via `ViewportBuilder::with_icon`

## Naming glossary (canonical — do not drift)

**Regions (top-level screen areas):**
- **Left Panel** — projects tree
- **Main Panel** — active Tab's Layout of Panes
- **Right Panel** — Changes / Files

**Hierarchy:**
- **Project** — a git repo on disk. Contains 1+ Workspaces.
- **Workspace** — a branch checkout of a Project, backed by `git worktree add` under `~/.crane-worktrees/<project-name>/<branch>` by default (user can override location). Contains 1+ Tabs.
- **Tab** — a named surface in the Main Panel. Owns one Layout.
- **Layout** — the split tree inside a Tab (what ⌘D / ⌘⇧D splits).
- **Pane** — a leaf in a Layout; one of: Terminal, Files, Markdown, Diff, Browser.
- **File Tab** — an open file inside the Files Pane. Internal-only term — never at the top level.

Code still uses the old names `Workspace` (for the Layout struct) and `Worktree` (for the Workspace struct); rename is agreed but not yet executed.

## Architecture

**Single binary, single process.** No FFI, no Go, no subprocesses other than `git`.

```
src/
├── main.rs          — eframe entry + shortcuts + top-level layout composition + modal
├── state.rs         — App + Project + Worktree (→ Workspace) + Tab, active focus
├── workspace.rs     — Layout tree (Node::Leaf / Node::Split), Pane, PaneContent enum
├── terminal/
│   ├── term.rs      — PTY spawn, crane_term::Term wiring, reader thread, input write
│   └── view.rs      — grid renderer via egui::Painter, key → escape sequence
├── (crates/crane_term/) — VT parser glue, grid, scrollback, reflow, sync replay
├── terminal_view.rs — grid renderer via egui::Painter, key → escape sequence
├── pane_view.rs     — renders Layout tree, headers, borders, splitters, focus
├── ui_left.rs       — Left Panel (project tree, + workspace, × remove, add project)
├── ui_right.rs      — Right Panel (Changes grouped tree, Files FS tree)
├── ui_top.rs        — Main Panel top bar (panel toggles, breadcrumb, action buttons)
├── git.rs           — shell-out git: status, stage, unstage, commit, push,
│                     worktree list/add, head_content, list_local_branches
└── views/
    ├── file_view.rs     — Files Pane (internal File Tabs + syntect)
    ├── markdown_view.rs — Markdown Pane (pulldown-cmark → egui RichText)
    ├── diff_view.rs     — Diff Pane (similar TextDiff)
    └── browser_view.rs  — Browser Pane (placeholder: URL + "Open in System Browser"; wry WebView still pending)
```

## Build / run

```bash
cargo build           # debug build (opt-level=1 for first-party, 3 for deps — fast enough to iterate)
cargo run             # run debug build
cargo build --release # release build for actual use
make test             # cargo test --bin crane
```

Keep `opt-level = 1` for `[profile.dev]` and `opt-level = 3` for `[profile.dev.package."*"]` — without these the GUI is noticeably laggy.

## Release workflow

**Always use the Makefile targets — never hand-roll `sed` / raw `git tag` / raw `gh release create`.** Hand-rolling has caused versions to ship without a DMG or without a git tag, which silently breaks the update checker and the “download latest” link.

One-shot release (recommended):

```bash
make ship                  # patch bump (0.1.x → 0.1.x+1)   — daily UX fixes
make ship-minor            # minor bump (0.1.x → 0.2.0)     — user-noticeable features
make ship-major            # major bump (0.1.x → 1.0.0)     — breaking / milestone
make ship-universal        # patch bump, universal (arm64 + x86_64) DMG
```

Each target: bump Cargo.toml → commit → release DMG → tag vX.Y.Z → push main + tag → upload DMG to GitHub. No hand-steps.

Step-by-step when you want to inspect between phases:

```bash
make bump-patch            # or bump-minor / bump-major
make release               # bundle → DMG (ad-hoc signed if DEVELOPER_ID unset)
make tag                   # tags vX.Y.Z from Cargo.toml, pushes main + tag
make upload TAG=v0.1.73    # attaches the DMG to a new GitHub release
```

Bump discipline:

- `bump-patch` — UX fixes / small additions — the common case
- `bump-minor` — features users notice (e.g. new shortcut, new pane) — `0.1.x → 0.2.0`
- `bump-major` — breaking change or a milestone — `0.x → 1.0.0`
- All three refuse to release on a dirty tree. Commit or stash first.
- **Never** hand-edit Cargo.toml + run `cargo build + gh release create`. The Makefile targets handle version evaluation via recursive make so the DMG filename and tag stay consistent. Hand-rolling has silently landed versions without a tag or DMG.

Notarized builds (Developer ID + Apple notary):

```bash
make setup-notary          # one-time: stores app-specific password in keychain
DEVELOPER_ID="..."  make signed-dmg             # sign → DMG → notarize → staple
DEVELOPER_ID="..."  make signed-dmg-universal
```

## Dependency rules

- **No async runtime.** PTY reader uses `std::thread`; egui wakes via `Context::request_repaint()`.
- **No `git2` / `libgit2`.** Always `Command::new("git").args(…).output()` — matches superset v2 host-service patterns.
- **No feature flags / backward-compat shims.** Change the code.
- **Package age policy**: global npm/bun/pnpm/uv configs enforce a 7-day minimum release age. Same rule applies here — pick an older stable version rather than bypassing.
- **Cargo.lock is gitignored** (consistent with existing `.gitignore`). App is binary, but the user chose to ignore it.

## UI rules

- **Naming**: use the canonical terms above everywhere (code, commit messages, comments, docstrings, UI strings). Call out drift.
- **Red ID-clash markers**: every `egui::ScrollArea` in a reusable widget needs `.id_salt(…)` and repeating rows need `ui.push_id((key, id), …)`.
- **Cursor icons**: plain `ui.label(…)` picks the text cursor — for clickable text use `Label::new(…).sense(Sense::click())` and `ctx.set_cursor_icon(CursorIcon::PointingHand)` on hover.
- **Inner pane padding**: panes get a 5×3px interior shrink so content doesn't kiss the border.
- **Focus border**: 2px accent on the active Pane; other Panes get a subtle border.
- **Panel toggles**: visible buttons in the Main Panel top bar for both Left and Right Panel collapse.
- **Icons — NEVER use Unicode glyphs (▲ ▼ ✕ ▎ · 🔍 etc.)** in buttons or text. Our bundled `JetBrains Mono` + `egui` default proportional font don't cover those ranges, so they render as tofu boxes. **Always use `egui_phosphor::regular::*`** (examples: `ARROW_UP`, `ARROW_DOWN`, `X_CIRCLE`, `MAGNIFYING_GLASS`, `FLOPPY_DISK`, `FOLDER_OPEN`, `COPY`, `PENCIL_SIMPLE`, `EYE`, `X`, `PLUS`, `MINUS`, `CARET_UP`, `CARET_DOWN`, `GIT_BRANCH`, `GIT_DIFF`, `INFO`, `WARNING`, `X_CIRCLE`, `CUBE`, `ARROW_RIGHT`, `ARROW_COUNTER_CLOCKWISE`, `DOWNLOAD_SIMPLE`, `FILE`, `FOLDER`, `FOLDER_PLUS`, `PAINT_BRUSH`, `CODE`, `TERMINAL_WINDOW`, `KEYBOARD`, `LIGHTNING`, `ARROW_CIRCLE_UP`, `TRASH`).
- **`request_focus()` must fire once, not every frame.** Per-frame `resp.request_focus()` steals clicks from sibling buttons in the same row — the TextEdit reclaims focus before the button processes its click. Gate with an egui memory flag keyed by the widget path and reset it when the container closes.
- **Close buttons live on the far right.** For toolbars / find bars / modal headers: pin the close × with `ui.with_layout(Layout::right_to_left(Align::Center), |ui| { … })` so the row stays visually anchored.
- **Icon buttons need `min_size`** (≥ 22×22) — egui sizes by text, and a single-glyph button can collapse into an invisible hitbox.

## Keyboard (canonical)

- `Cmd+T` — split active Pane with a new terminal
- `Cmd+Shift+T` — new Tab in active Workspace
- `Cmd+D` — split Pane horizontally with new terminal
- `Cmd+Shift+D` — split Pane vertically with new terminal
- `Cmd+W` — close focused Pane
- `Cmd+Shift+W` — close active Tab
- `Cmd+[` / `Cmd+]` — focus prev / next Pane
- `Cmd+B` / `Cmd+/` — toggle Left / Right Panel
- `Cmd+=` / `Cmd+-` / `Cmd+0` — font size up / down / reset

## Git / commit rules

- **Commit messages: zero AI references.** No "Claude", no "Co-Authored-By", no bot/assistant mentions. Every commit must pass as human work. This is inherited from `~/.claude/CLAUDE.md` and is non-negotiable.
- **Prefer `gh` CLI** for PRs / issues / checkout where possible.
- **Never force-push main.** Never skip hooks (`--no-verify`) without explicit request.
- **Conventional commits** matching the existing style in `superset` monorepo: `feat:`, `fix:`, `chore:`, `refactor:`, etc.
- **Crane repo remote**: `https://github.com/rajpootathar/Crane.git`. Pushes go there, not to the enclosing `superset` monorepo.

## Memory / persistence

Agent memory lives at `~/.claude/projects/-Users-rajpootathar-ideaProjects-superset/memory/`:
- `project_crane_naming.md` — canonical glossary (above)
- `project_crane_config_persistence.md` — font size / themes via `crane.yaml`

User-facing persistence: `~/.crane/` (planned for config, sessions, themes). Not yet implemented.

## Known issues

- **Cursor drifts a few columns short of `%` with certain custom prompts.**
  Root cause is external: some zsh prompt frameworks (observed with
  Forge theme; also known to affect older Powerlevel10k) compute their
  RPROMPT cursor-back escape against UTF-8 **byte width** instead of
  **column width** for Nerd-Font / PUA glyphs. Each 3-byte icon
  over-counts by 2 cells, so the cursor lands `2 × icon_count` cells
  short of the prompt end. Crane's VT grid is correct; the shell is
  writing the wrong `\e[<n>D`. Workaround: disable the offending
  prompt theme or switch its icon set to ASCII. Crane cannot repair
  this from the terminal side without lying about grid width.

- **Older scrollback rows imperfectly reflow on width changes.**
  After multi-resize sequences, rows that pre-date the
  WRAPLINE-on-actual-wrap logic may not perfectly unwrap when the
  terminal is widened back. Recent content (typed after the fix
  landed) reflows correctly. Recoverable by a fresh shell `clear`
  — the grid surface beyond the visible viewport is just
  presentation state, not real content. Tracked under
  `crane_term::reflow` v2: extend reflow to recover from older
  rows that lack the full WRAPLINE chain.

- **Diagnostic probe**: `CRANE_VT_TRACE=1` dumps raw PTY bytes to
  `~/.crane/vt-trace-<pid>.log` for offline replay. The
  `?2026` Synchronized Output duplicate-prompt bug is fixed in
  `crane_term` at the Processor/Term layer (sync-frame replay
  suppresses scrollback eviction). 38 unit tests in
  `crates/crane_term` pin the behavior.

## Pending major work

- Rename `Workspace` → `Layout`, `Worktree` → `Workspace` throughout the code
- `crane_term` v2 reflow: walk WRAPLINE chains across the live-grid / scrollback boundary so older scrollback rows reflow on multi-resize the same way recent rows do
- Drag-drop Pane rearrange in Layout tree
- `wry`-backed embedded browser Pane (currently a placeholder)
- Session save/restore (`~/.crane/sessions/`)
- Config + theme loading (`crane.yaml`, hot-reload)
- Mouse selection + copy/paste in Terminal Panes

## Out of scope for v1

- Multi-user / team collaboration
- Plugin/extension system
- Custom GPU-rendered text editor (Files Pane uses egui RichText + syntect; no editing)
- Agent orchestration
- Windows + Linux polish (macOS-first, but cross-platform deps are selected)
