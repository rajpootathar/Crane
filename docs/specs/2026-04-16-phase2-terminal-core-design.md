# Phase 2: Terminal Core — Design Spec

**Date:** 2026-04-16
**Status:** Approved

---

## Goal

Turn the blank Crane window into a working GPU-rendered terminal emulator. The full keystroke loop: input → PTY → shell → output → VT parse → GPU render.

## Architecture

```
OS key → winit → on_input callback → Go PTY.Write()
                                          ↓
Shell output ← kernel PTY ← shell process
     ↓
Go PTY.Read() → crane_write_terminal() → Rust
     ↓
alacritty_terminal VT parse → grid cells
     ↓
crane-text glyph atlas → crane-renderer GPU draw → pixels
```

## Components

### crane-text (Rust)
- Load monospace font via cosmic-text (bundle JetBrains Mono, fallback to system monospace)
- Calculate cell metrics (cell_width, cell_height, baseline) from font at configured size
- Glyph atlas: rasterize glyphs on demand via swash, pack into a dynamically-growing GPU texture
- API: `FontSystem::new(font_family, font_size) -> (CellMetrics, GlyphAtlas)`
- API: `GlyphAtlas::get_or_insert(char, style) -> GlyphInfo { uv_rect, offset }`

### crane-terminal (Rust)
- Wraps `alacritty_terminal` crate's `Term` type
- One `TerminalInstance` per pane, runs VT parsing on its own OS thread
- Double-buffered: terminal thread writes to grid, render reads snapshot
- `TerminalInstance::write(bytes)` — feed output from PTY into VT parser
- `TerminalInstance::snapshot() -> Vec<RenderableCell>` — lock-free read of current grid
- `RenderableCell`: { column, line, character, fg_color, bg_color, flags (bold/italic/underline) }
- `TerminalInstance::resize(cols, rows)` — resize the internal grid

### crane-renderer (extended)
- New `TerminalPipeline`: wgpu render pipeline for terminal grid
- Vertex shader: instanced quads positioned by (col, row) grid coordinates
- Fragment shader: samples glyph atlas texture, applies foreground color
- Background pass: solid color quads per cell (only where bg != default)
- Cursor: colored rect at cursor position (block style for Phase 2)
- Per-frame: read terminal snapshot → build instance buffer → draw

### Go internal/pty (new package)
- `PtyManager`: create/destroy PTY sessions, map pane_id → PtySession
- `PtySession`:
  - Spawns shell process with PTY (creack/pty)
  - Read goroutine: reads from PTY fd → calls `ffi.WriteTerminal(pane_id, data)`
  - Write channel: bounded `chan []byte` (1MB cap), backpressure on fast input
  - Resize: `ioctl(TIOCSWINSZ)` with debounce
- Shell detection: query `$SHELL` env, fallback to `/bin/zsh` then `/bin/bash`

### Go internal/app (extended)
- On `crane_init`: create one PTY session (pane_id=1), auto-activate
- Wire callbacks:
  - `on_input` → `ptyManager.Write(pane_id, data)`
  - `on_pane_resize` → `ptyManager.Resize(pane_id, cols, rows)`
- On window close: kill all PTY sessions

### FFI additions
- `crane_write_terminal(handle, pane_id, data, len)` — Go→Rust: feed PTY output to terminal
- `crane_resize_pane(handle, pane_id, cols, rows)` — Go→Rust: resize terminal grid
- `crane_query_font_metrics(handle, font_name, size)` — Rust→Go: return cell dimensions as JSON
- Callbacks: `on_input`, `on_pane_resize` now wired to Go

## Non-Goals (Phase 2)
- Split panes, sidebars, breadcrumbs (Phase 3)
- Scrollback save/restore (Phase 5)
- Selection, copy/paste, OSC 52
- Ligatures
- Multiple font sizes / hot-reload
- Image protocols (sixel, kitty)
