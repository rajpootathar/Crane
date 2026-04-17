# Phase 2: Terminal Core — Implementation Plan

**Goal:** Working GPU-rendered terminal emulator in the Crane window.

**Spec:** `crane/docs/specs/2026-04-16-phase2-terminal-core-design.md`

---

## Task 1: crane-text — Font loading + glyph atlas

- [ ] Add cosmic-text + swash dependencies
- [ ] Create FontSystem, load system monospace font
- [ ] Calculate cell metrics (cell_width, cell_height, baseline)
- [ ] Build GlyphAtlas: HashMap<CacheKey, GlyphInfo> + pixel buffer
- [ ] Rasterize glyphs on demand via SwashCache
- [ ] Pack into a texture atlas (simple row packing, grow as needed)
- [ ] Expose: CellMetrics, GlyphAtlas, get_or_rasterize()
- [ ] Verify: unit test that loads font and rasterizes 'A'

## Task 2: crane-terminal — alacritty_terminal wrapper

- [ ] Add alacritty_terminal + vte dependencies
- [ ] Create TerminalInstance wrapping Term<VoidListener>
- [ ] Implement Dimensions trait for terminal size
- [ ] write(bytes) → processor.advance() to feed PTY output
- [ ] snapshot() → read renderable_content(), collect RenderableCell vec
- [ ] resize(cols, rows) → term.resize()
- [ ] Define RenderableCell: { col, line, c, fg, bg, flags }
- [ ] Thread safety: Mutex<Term> for concurrent access
- [ ] Verify: unit test that writes "Hello" and reads cells back

## Task 3: crane-renderer — Terminal GPU pipeline

- [ ] Write WGSL vertex + fragment shaders for terminal grid
- [ ] Create terminal render pipeline (vertex buffer layout, bind groups)
- [ ] Upload glyph atlas as wgpu texture
- [ ] Per-frame: build instance buffer from RenderableCell data
- [ ] Background pass: colored quads for cell backgrounds
- [ ] Foreground pass: textured quads sampling glyph atlas
- [ ] Cursor rendering (block rect at cursor position)
- [ ] Wire into existing render_frame() flow
- [ ] Verify: hardcode "Hello World" cells, see text GPU-rendered

## Task 4: Wire crane-text + crane-terminal into crane-app

- [ ] AppHandle creates FontSystem + GlyphAtlas on init
- [ ] AppHandle creates TerminalInstance (80x24 default)
- [ ] crane_write_terminal FFI: forward bytes to TerminalInstance
- [ ] crane_resize_pane FFI: forward resize to TerminalInstance
- [ ] Per frame: snapshot terminal → build render data → draw
- [ ] Verify: hardcode writing "ls\n" to terminal, see parsed output render

## Task 5: Go PTY manager

- [ ] Add github.com/creack/pty dependency
- [ ] internal/pty/manager.go: PtyManager with create/destroy
- [ ] internal/pty/session.go: PtySession with read loop + write channel
- [ ] Detect login shell ($SHELL, fallback /bin/zsh, /bin/bash)
- [ ] Read loop: pty.Read() → ffi.WriteTerminal(pane_id, data)
- [ ] Write: bounded chan []byte, drain to pty.Write()
- [ ] Resize: Setsize with debounce
- [ ] Verify: go build compiles

## Task 6: Wire Go callbacks + FFI

- [ ] Implement crane_write_terminal in crane-ffi
- [ ] Implement crane_resize_pane in crane-ffi
- [ ] Implement crane_query_font_metrics in crane-ffi
- [ ] Wire on_input callback: Go receives keystrokes → ptySession.Write()
- [ ] Wire on_pane_resize callback: Go receives resize → ptySession.Resize()
- [ ] Go app.go: on init, query font metrics, calculate cols/rows, create PTY
- [ ] Update Go bridge.go with new FFI functions and callbacks

## Task 7: End-to-end integration + verification

- [ ] make build succeeds
- [ ] make run opens window with working terminal
- [ ] Can type commands and see output
- [ ] Colors render correctly (ls --color)
- [ ] Resize window → terminal resizes
- [ ] Ctrl+C, Ctrl+D work
- [ ] Fix any issues
