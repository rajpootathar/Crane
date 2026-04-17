# Crane — Architecture Design Spec

**Date:** 2026-04-15
**Status:** Approved
**Author:** rajpootathar

---

## 1. Overview

Crane is a native, GPU-rendered desktop development environment for orchestrating terminals, editors, browsers, and git workflows across isolated git worktrees. It replaces Electron-based tools with a Go + Rust hybrid architecture: Go manages process lifecycle, PTY, git, and state; Rust handles windowing, GPU rendering, terminal emulation, and text shaping via FFI.

### Goals

- Native performance: ~15-25MB RAM, ~50-100ms startup
- GPU-rendered terminal with crash isolation per pane
- Split pane system supporting terminal, browser, editor, diff, file explorer, markdown, image viewer
- Tree sidebar: Project → Worktree → Tab (with diff stat badges)
- Right sidebar: quick git operations (stage/commit/push) + file browser
- Cross-platform: macOS + Linux + Windows from day one
- Solo developer focus for v1
- Polished product launch (Show HN)

### Non-Goals (v1)

- Multi-user / team collaboration
- Agent orchestration (requires backend infra — deferred to v2)
- Custom GPU-rendered text editor (use CodeMirror in webview for v1)
- Plugin/extension system

---

## 2. Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend (orchestration) | Go 1.22+ |
| Renderer (GPU) | Rust (compiled to shared library, loaded via CGo FFI) |
| Windowing | winit (Rust) |
| GPU | wgpu (Metal / Vulkan / DX12 / OpenGL) |
| Terminal emulation | alacritty_terminal crate |
| Text shaping | cosmic-text + swash |
| Browser panes | System webview (WKWebView / WebView2 / WebKitGTK) |
| PTY management | go-pty (creack/pty on Unix, ConPTY on Windows) |
| Git operations | Shell out to git binary |
| File watching | fsnotify |
| Config format | YAML (crane.yaml) |
| Theme format | YAML, fully customizable, hot-reloadable |
| Keybindings | Standard shortcuts (Cmd/Ctrl), fully remappable via YAML |

---

## 3. UI Layout

```
┌──────────────┬──────────────────────────┬──────────────┐
│ LEFT SIDEBAR │    CENTER CONTENT        │ RIGHT SIDEBAR│
│   (200px)    │                          │   (220px)    │
│              │  Breadcrumb bar          │              │
│ Tree:        │  ─────────────────────   │ Tabs:        │
│ Project      │  ┌─────────┬────────┐   │ [Changes]    │
│  └ Worktree  │  │Terminal │Browser │   │ [Files]      │
│    └ Tab     │  │         │        │   │              │
│              │  ├─────────┴────────┤   │ Staged files │
│ Diff badges  │  │ Terminal 2       │   │ Unstaged     │
│ on worktrees │  └──────────────────┘   │              │
│              │                          │ [Commit box] │
│ + Add repo   │  Split pane canvas      │ [Commit] [Push]│
└──────────────┴──────────────────────────┴──────────────┘
```

### Left Sidebar — Navigation Tree

- **Project** (repo) → **Worktree** (branch) → **Tab** (layout/pane)
- Diff stat badges on worktrees (+/- counts)
- Smart collapse: only active worktree expands tabs, others show count badge
- Quick search via Cmd+K
- Add/remove repositories

### Center Content — Split Pane Canvas

- Recursive binary split tree (same data structure as tmux)
- Cmd+D split right, Cmd+Shift+D split down
- Drag borders to resize
- Any pane can be any type: terminal, browser, editor, diff, file explorer, markdown, image
- Breadcrumb bar showing: project > worktree > active tab
- Pane header showing type + context (e.g., "bun test --watch")

### Right Sidebar — Git & Files

- **Changes tab**: staged/unstaged/untracked files, stage/unstage actions, commit message box, commit + push buttons
- **Files tab**: full workspace file tree browser with create/rename/delete
- Toggle with Cmd+/
- Click a file → opens as a pane in center for deep work

### Pane Types (v1)

| Type | Rendering | Notes |
|------|-----------|-------|
| Terminal | GPU (alacritty_terminal + wgpu) | Own Rust thread per instance |
| Editor | Webview (CodeMirror 6) | Bundled HTML, syntax highlighting |
| Browser | System webview | WKWebView / WebView2 / WebKitGTK |
| Diff viewer | Webview or custom | Git diff with inline/side-by-side |
| File explorer | GPU (custom renderer) | Lightweight, main thread |
| Markdown preview | Webview | Render .md files |
| Image viewer | GPU (texture decode) | Decode once, display as texture |

### Preset Commands

Quick-launch terminals with pre-configured commands (e.g., "Run dev server" → `bun dev`). Defined in `crane.yaml`. No special pane type — just terminals with a command pre-filled.

---

## 4. Process & Thread Model

### Single Process Architecture

```
Single OS Process
├── Main Thread (Rust)
│   └── winit event loop → wgpu render loop → compositor
│
├── Go Runtime
│   ├── goroutine: App event loop (lifetime: app)
│   ├── goroutine: PTY read loop × N (lifetime: pane active)
│   ├── goroutine: PTY write loop × N (lifetime: pane active)
│   ├── goroutine: Git watcher per repo (lifetime: repo open)
│   ├── goroutine: Config watcher (lifetime: app)
│   └── goroutine: Session save workers (lifetime: close event)
│
├── Rust Terminal Threads × N
│   ├── thread: terminal-1 (VT parser + grid) ← RwLock → compositor
│   ├── thread: terminal-2 (independent, crash-isolated)
│   └── thread: terminal-N
│
└── OS-managed
    └── System webview processes (browser/editor panes)
```

### Crash Isolation

Each Rust terminal thread is wrapped in `std::panic::catch_unwind`. A panic in one terminal:
1. Marks the pane as `ErrorState`
2. Sends callback to Go: `on_layout_action("pane_crashed", pane_id)`
3. Thread exits cleanly
4. Go tears down the PTY, marks pane Ghost, re-renders with error UI
5. User can click to restart

### Pane Lifecycle States

```
Ghost (metadata only, ~0 memory)
  │  user clicks tab
  ▼
Active (thread + PTY running, GPU rendering)
  │  user navigates away
  ▼
Suspended (scrollback saved, thread paused, no GPU)
  │  user returns
  ▼
Active (~100ms to wake)
```

On app launch: all panes start as Ghost. Only the last-active pane is activated. Everything else wakes on click.

### Synchronization Primitives

| Resource | Primitive | Reason |
|----------|-----------|--------|
| Layout tree | `sync.RWMutex` + `atomic.Value` | Lock-free reads, locked writes |
| PTY map | `sync.Map` | Concurrent pane creation during restore |
| Terminal grid | `Arc<Mutex<TerminalGrid>>` double-buffer | Render reads, terminal writes |
| PTY write queue | `chan []byte` (bounded 1MB) | Backpressure on fast input |
| Rust command queue | `EventLoopProxy<AppCommand>` | Cross-thread command delivery |
| IME state | `sync/atomic.Bool` per PTY | Gate key delivery during composition |

---

## 5. FFI Boundary

### Three-Layer Contract

**Layer 1 — Init/Teardown (called once)**

```c
CraneHandle crane_init(const char *config_json, CraneCallbacks callbacks);
void crane_shutdown(CraneHandle handle);
bool crane_set_theme(CraneHandle handle, const char *theme_json);
```

**Layer 2 — Commands (Go → Rust)**

```c
bool crane_apply_layout(CraneHandle handle, const char *layout_json);
bool crane_write_terminal(CraneHandle handle, uint32_t pane_id, const uint8_t *data, uintptr_t len);
bool crane_resize_pane(CraneHandle handle, uint32_t pane_id, uint16_t cols, uint16_t rows);
bool crane_set_pane_state(CraneHandle handle, uint32_t pane_id, uint8_t state);
bool crane_set_breadcrumb(CraneHandle handle, const char *text_json);
bool crane_open_url(CraneHandle handle, uint32_t pane_id, const char *url);
const char *crane_query_font_metrics(CraneHandle handle, const char *font_name, float size);
void crane_set_ime_state(CraneHandle handle, uint32_t pane_id, bool composing);
uint8_t *crane_capture_screenshot(CraneHandle handle, uint32_t pane_id, uintptr_t *out_len);
uint8_t *crane_capture_scrollback(CraneHandle handle, uint32_t pane_id, uintptr_t *out_len);
bool crane_replay_scrollback(CraneHandle handle, uint32_t pane_id, const uint8_t *data, uintptr_t len);
void crane_free_bytes(uint8_t *ptr, uintptr_t len);
```

**Layer 3 — Callbacks (Rust → Go)**

```c
typedef struct {
    void (*on_input)(uint32_t pane_id, const uint8_t *data, uintptr_t len, void *ctx);
    void (*on_pane_focus)(uint32_t pane_id, void *ctx);
    void (*on_pane_resize)(uint32_t pane_id, uint16_t cols, uint16_t rows, void *ctx);
    void (*on_layout_action)(const char *action_json, void *ctx);
    void (*on_window_event)(const char *event_json, void *ctx);
    void *ctx;
} CraneCallbacks;
```

### Key Design Decision

`crane_apply_layout` receives the entire layout tree as JSON on every structural change. Rust diffs against its cached tree and creates/destroys pane surfaces accordingly. This makes every layout mutation atomic — no ordering bugs from separate create/split/destroy calls. At <50 panes, the JSON is under 10KB and serialization is under 1ms.

---

## 6. Rust Crate Structure

```
crane/crates/
├── Cargo.toml              # Workspace manifest
├── crane-ffi/              # C-ABI boundary, compiles to .dylib/.so/.dll
│   └── src/lib.rs           # #[no_mangle] extern "C" exports, catch_unwind
├── crane-app/              # AppHandle, command loop, callback registry
│   └── src/lib.rs
├── crane-window/           # winit event loop, OS window, IME, high-DPI
│   └── src/lib.rs
├── crane-layout/           # Pure data: pane tree, split geometry, rect math
│   └── src/lib.rs           # LayoutNode, compute_rects, diff_layout
├── crane-renderer/         # wgpu device, frame loop, pane surface compositing
│   └── src/lib.rs
├── crane-terminal/         # alacritty_terminal integration, per-thread grid
│   └── src/lib.rs           # TerminalInstance, RenderCell, double-buffer
├── crane-text/             # cosmic-text font shaping, glyph atlas, cell metrics
│   └── src/lib.rs
├── crane-webview/          # Platform webview wrappers (WKWebView/WebView2/WebKitGTK)
│   └── src/lib.rs           # WebviewSurface trait + platform impls
├── crane-theme/            # YAML theme parsing, hot-reload via notify
│   └── src/lib.rs
└── crane-proto/            # Shared types: IDs, enums, JSON payloads (no unsafe)
    └── src/lib.rs
```

### Crate Dependency Graph

```
crane-ffi → crane-app → crane-window
                       → crane-layout
                       → crane-renderer → crane-text
                                        → crane-terminal
                       → crane-webview
                       → crane-theme
          → crane-proto (used by all crates)
```

### Key Crate Details

**crane-layout** — Pure data, no rendering, no OS calls, fully unit-testable:
```rust
pub enum LayoutNode {
    Split { id: NodeId, direction: Direction, sizes: Vec<f32>, children: Vec<LayoutNode> },
    Group { id: NodeId, active_pane: Option<PaneId>, panes: Vec<PaneDescriptor> },
}
```

**crane-terminal** — One `TerminalInstance` per active terminal pane, each in its own OS thread (not tokio). Double-buffered grid with `Arc<Mutex<>>` for lock-free rendering.

**crane-renderer** — Immediate-mode UI chrome (rects, text runs, icons). No egui/imgui — the UI is fixed-layout enough that a custom ~200-line renderer beats integration cost.

**crane-webview** — Common trait `WebviewSurface` with platform implementations selected via `#[cfg(target_os)]` at compile time.

---

## 7. Go Module Structure

```
crane/go/
├── go.mod                          # module crane
├── cmd/crane/
│   └── main.go                     # Entry point
├── internal/
│   ├── ffi/
│   │   ├── bridge.go               # CGo declarations, //export callbacks
│   │   └── types.go                # Go mirror of crane-proto JSON types
│   ├── pty/
│   │   ├── manager.go              # PtyManager: create/destroy/resize
│   │   ├── session.go              # PtySession: read loop, write queue
│   │   └── writequeue.go           # Bounded write channel, backpressure
│   ├── layout/
│   │   ├── tree.go                 # LayoutNode tree (Go-side mirror)
│   │   ├── store.go                # LayoutStore: mutex, version counter, apply
│   │   └── actions.go              # SplitPane, ClosePane, ActivatePane etc.
│   ├── session/
│   │   ├── manager.go              # Ghost/Active/Suspended lifecycle
│   │   ├── restore.go              # Save/load JSON + scrollback
│   │   └── types.go                # SessionRecord, PaneLifecycle enum
│   ├── git/
│   │   ├── status.go               # git status parsing
│   │   ├── operations.go           # stage, commit, push, pull
│   │   ├── watcher.go              # fsnotify on .git/index, HEAD, refs/
│   │   └── types.go                # GitStatus, FileChange
│   ├── config/
│   │   ├── loader.go               # YAML parse, fsnotify hot-reload
│   │   ├── schema.go               # Config struct
│   │   └── defaults.go             # Default values
│   ├── keybind/
│   │   ├── registry.go             # Keybinding registry, chord matching
│   │   └── defaults.go             # Default keybinding table
│   └── app/
│       ├── app.go                  # App struct: owns all managers
│       ├── handlers.go             # Dispatches Rust callbacks
│       └── commands.go             # Outbound command queue to Rust
```

### Package Rules

- **internal/ffi** — the ONLY package touching CGo. All `//export` functions in `bridge.go`. No business logic.
- **internal/git** — shells out to `git` binary via `exec.Command`. No go-git, no libgit2.
- **internal/layout** — the ONLY package calling `ffi.ApplyLayout`. Single source of layout truth.
- **internal/app** — the ONLY coordinator between managers. Processes `chan AppEvent`.

---

## 8. Data Flows

### Keystroke in Terminal

```
OS keyboard → winit → crane-ffi callback → Go go_on_input()
→ app.HandleInput() → pty.Write(data) → kernel PTY
→ shell output → pty.Read() → ffi.WriteTerminal()
→ crane-terminal VT parse → grid update → dirty flag
→ crane-renderer next frame → GPU draw → pixels
```

### Split Pane Creation

```
Cmd+D → winit → on_layout_action("split_requested") → Go
→ layout.Apply(SplitPane) → serialize tree → ffi.ApplyLayout(json)
→ Rust diff_layout() → create Ghost PaneSurface
→ user clicks Ghost → on_pane_focus → Go session.Activate()
→ pty.Create() + ffi.SetPaneState(Active) → thread spawns → rendering
```

### Git Commit from Right Sidebar

```
Click "Commit" → on_layout_action("commit_requested") → Go
→ git.Commit(repo, message) → exec.Command("git", "commit", "-m", msg)
→ git.GetStatus() → layout.Apply(SetRightPanel{newStatus})
→ ffi.ApplyLayout() → Rust re-renders right panel
```

### Session Save/Restore

```
Save: window close → Go saves each pane concurrently (2s timeout)
  → crane_capture_scrollback → write to ~/.config/crane/sessions/
  → session.json with schema version

Restore: launch → read session.json → all panes Ghost
  → activate last-active pane only → crane_replay_scrollback
  → everything else wakes on click (~100ms)
```

### Theme Hot-Reload

```
User edits crane.yaml → fsnotify → 200ms debounce → re-parse
→ diff old vs new theme → ffi.SetTheme(json)
→ Rust re-uploads color uniforms → next frame renders new colors
```

---

## 9. Configuration

### crane.yaml

```yaml
# ~/.config/crane/crane.yaml

shell:
  default: auto          # auto-detect login shell
  env:
    EDITOR: nvim

terminal:
  scrollback: 10000
  font:
    family: "JetBrains Mono"
    size: 13
  cursor:
    style: block
    blink: true

keybindings:
  split_right: "Cmd+D"
  split_down: "Cmd+Shift+D"
  close_pane: "Cmd+W"
  toggle_sidebar: "Cmd+B"
  toggle_right_panel: "Cmd+/"
  search: "Cmd+K"
  focus_next_pane: "Cmd+]"
  focus_prev_pane: "Cmd+["
  new_terminal: "Cmd+T"

commands:
  - name: "Dev Server"
    cmd: "bun dev"
    cwd: "."
  - name: "Watch Tests"
    cmd: "bun test --watch"
  - name: "Docker Logs"
    cmd: "docker compose logs -f"

theme: "crane-dark"      # or path to custom theme YAML
```

### Theme YAML

```yaml
# ~/.config/crane/themes/crane-dark.yaml
name: crane-dark
colors:
  background: "#0e1018"
  foreground: "#b0b4c0"
  accent: "#e8922a"
  accent_secondary: "#5a7abf"
  success: "#4a9"
  warning: "#c08040"
  error: "#c55"
  border: "#1e2030"
  surface: "#14161e"
  surface_elevated: "#1a1c28"
  text_muted: "#4a4c5a"

  # ANSI terminal colors
  black: "#1a1c28"
  red: "#c55"
  green: "#4a9"
  yellow: "#e8922a"
  blue: "#5a7abf"
  magenta: "#a06"
  cyan: "#5aa"
  white: "#b0b4c0"
  # ... bright variants

fonts:
  mono: "JetBrains Mono"
  ui: "Inter"

syntax:
  keyword: "#c08040"
  string: "#4a9"
  comment: "#4a4c5a"
  function: "#5a7abf"
  type: "#e8922a"
  number: "#a06"
```

---

## 10. Build System

### Directory Layout

```
crane/
├── go/                     # Go module
├── crates/                 # Rust workspace
├── include/
│   └── crane.h             # C header for CGo
├── Makefile
├── crane.yaml              # Default config (go:embed)
└── .github/workflows/
    ├── build.yml
    └── release.yml
```

### Build Flow

```bash
# Step 1: Rust → shared library
cargo build --release -p crane-ffi
# → target/release/libcrane_ffi.dylib (macOS)
# → target/release/libcrane_ffi.so (Linux)
# → target/release/crane_ffi.dll (Windows)

# Step 2: Go → binary (links the .dylib)
CGO_ENABLED=1 \
CGO_CFLAGS="-I../include" \
CGO_LDFLAGS="-L../crates/target/release -lcrane_ffi" \
go build -o crane ./cmd/crane
```

### Output

```
crane                       (~15MB Go binary)
libcrane_ffi.dylib          (~8MB Rust library)
────────────────────
Total: ~23MB
```

### CI Matrix

- **macOS** (macos-14, arm64): Universal binary via lipo, codesign + notarize
- **Linux** (ubuntu-22.04): Install libwebkit2gtk-4.1-dev, AppImage output
- **Windows** (windows-2022): WebView2 SDK, NSIS installer

---

## 11. Known Gaps & Mitigations

| Gap | Mitigation |
|-----|-----------|
| Scrollback capture/replay | Add `crane_capture_scrollback` + `crane_replay_scrollback` FFI functions |
| Windows PTY | Use `go-pty` (abstracts creack/pty + ConPTY) |
| Editor pane | CodeMirror 6 in webview for v1, custom GPU editor later |
| Clipboard OSC 52 | Detect in crane-terminal VT listener, callback to Go |
| Shell detection | Query login shell from OS, source profile before spawn |
| Resize debouncing | Throttle `ioctl(TIOCSWINSZ)` to 60ms in Go |
| Session JSON versioning | Schema `version` field + migration function from day one |
| High-DPI webview coords | Layout carries both physical_rect and logical_rect |
| Font metrics ordering | `crane_query_font_metrics` must be called before first PTY creation |
| Crash recovery | Fall back to empty session on corrupt/truncated session.json |
| Drag-to-resize hit testing | Rust detects mouse-down within 4px of split boundary, sends resize callbacks |
| Git status reactivity | Wire GitWatcher → AppEvent → LayoutStore.Apply → crane_apply_layout |

---

## 12. Implementation Phases

### Phase 1 — Skeleton (Weeks 1-3)

- Initialize Rust workspace with all 9 crates as empty libs
- Initialize Go module
- Create `crane.h` C header with full FFI surface
- Implement crane-proto (shared types)
- Implement crane-ffi stubs + crane-window (winit blank window)
- Go CGo wrappers in internal/ffi
- Makefile build targets work on macOS
- **Deliverable:** `go build` produces binary that opens a blank GPU-rendered window

### Phase 2 — Terminal Core (Weeks 4-7)

- crane-text: font loading, cell metrics, glyph atlas
- crane-terminal: TerminalInstance, VT parse, per-thread isolation
- crane-renderer: wgpu pipeline for terminal grid, single pane fills window
- Go PTY manager + session + write queue
- Wire full keystroke loop: input → PTY → output → render
- Resize handling (debounced)
- **Deliverable:** Working terminal emulator in a native window

### Phase 3 — Layout Engine (Weeks 8-11)

- crane-layout: full tree model, compute_rects, diff_layout
- crane-renderer: multiple pane surfaces, split compositing
- Go layout store + actions
- crane_apply_layout drives pane creation/destruction
- Split divider hit-testing + drag-resize
- Left sidebar tree rendering
- Right sidebar rendering (placeholder)
- Breadcrumb bar
- **Deliverable:** Multiple split terminals with tree sidebar

### Phase 4 — Git Integration (Weeks 12-14)

- Go git status/operations/watcher
- Wire GitWatcher → layout updates → right sidebar
- Right sidebar: file change list, stage/unstage, commit, push
- Diff stat badges on worktree nodes in tree
- **Deliverable:** Functional git workflow from right sidebar

### Phase 5 — Session Management (Weeks 15-17)

- crane_capture_scrollback + crane_replay_scrollback
- Go session manager + restore logic
- Save on close, restore on launch (Ghost → Active)
- Screenshot capture for Ghost thumbnails
- Session JSON versioning
- **Deliverable:** Close and reopen with full state restoration

### Phase 6 — Browser + Editor Panes (Weeks 18-21)

- crane-webview: macOS WKWebView implementation
- crane_open_url navigation
- Webview frame positioning from layout rects
- Editor pane: webview + bundled CodeMirror 6
- Diff viewer pane
- Markdown preview pane
- File explorer pane
- Image viewer pane
- **Deliverable:** All 7 pane types functional

### Phase 7 — Config, Theme, Polish (Weeks 22-25)

- crane-theme: YAML parse, hot-reload
- Go config loader + hot-reload dispatch
- Default industrial dark theme
- Keybinding registry + remapping
- IME composition gating
- Clipboard OSC 52 support
- Linux: WebKitGTK, CI job
- Windows: ConPTY, WebView2, CI job
- macOS app bundle, codesign, notarize
- Performance profiling (target: 500MB/s terminal throughput)
- **Deliverable:** Cross-platform, polished, ready for Show HN

---

## 13. v2 Features (Post-Launch)

- Agent orchestration (spawn/monitor AI coding agents across worktrees)
- Custom GPU-rendered text editor (replace CodeMirror webview)
- Plugin/extension system
- Team collaboration (shared workspaces, real-time sync)
- Integrated task management
- SSH remote terminal support
- GPU-rendered diff viewer (replace webview)
- Tab previews on hover
- Command palette with fuzzy search
- Integrated search across all open projects
