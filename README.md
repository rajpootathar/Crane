# Crane

Native, GPU-rendered desktop development environment for orchestrating terminals, file browsing, diffs, and git workflows across isolated git workspaces.

Built in pure Rust on [egui](https://github.com/emilk/egui) + [wgpu](https://github.com/gfx-rs/wgpu), with [alacritty_terminal](https://github.com/alacritty/alacritty) driving the VT parser and [portable-pty](https://github.com/wez/wezterm/tree/main/pty) for cross-platform PTY.

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

- **Split panes** — horizontal / vertical splits with draggable dividers.
- **Pane types** — Terminal, Files (editable, tabbed, syntax-highlighted via syntect), Markdown, Diff, Browser (placeholder — native webview pending).
- **Left Panel** — Projects → Workspaces (git worktrees) → Tabs tree with diff-stat badges.
- **Right Panel** — Git Changes (stage / unstage / commit / push / pull) + Files browser.
- **Session restore** — projects, workspaces, tabs, layouts, open files, panel state all persist to `~/.crane/session.json` and restore on launch.
- **Git worktree management** — create a new workspace by picking a branch + location; removal cleans up.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Cmd+T` | Split active Pane with a new terminal |
| `Cmd+Shift+T` | New Tab in active Workspace |
| `Cmd+D` / `Cmd+Shift+D` | Split horizontally / vertically |
| `Cmd+W` | Close focused Pane |
| `Cmd+Shift+W` | Close active Tab |
| `Cmd+[` / `Cmd+]` | Focus prev / next Pane |
| `Cmd+B` / `Cmd+/` | Toggle Left / Right Panel |
| `Cmd+=` / `Cmd+-` / `Cmd+0` | Font size up / down / reset |
| `Cmd+S` | Save the active file in Files Pane |
| `Cmd+Enter` | Submit commit (when the message field is focused) |

---

## Build from source

Requires **Rust 1.94+** and platform-specific system dependencies.

```bash
git clone https://github.com/rajpootathar/Crane.git
cd Crane
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

Single binary. Pure Rust. No Electron, no web runtime, no FFI.

```
src/
├── main.rs          eframe entry + shortcuts + top-level composition
├── state.rs         App · Project · Workspace · Tab
├── layout.rs        Layout tree (Node::Leaf / Node::Split) · Pane · PaneContent
├── terminal.rs      PTY spawn + alacritty Term + reader thread + input writer
├── terminal_view.rs Grid renderer via egui::Painter; key → escape sequence
├── pane_view.rs     Renders Layout tree · headers · borders · splitters · focus
├── ui_left.rs       Left Panel (project tree)
├── ui_right.rs      Right Panel (Changes · Files)
├── ui_top.rs        Main Panel top bar (breadcrumb, action buttons, panel toggles)
├── ui_util.rs       Shared widget primitives + tree row + design tokens
├── git.rs           Shell-out git: status · stage · unstage · commit · push · pull · worktree
├── session.rs       Session save/restore (~/.crane/session.json)
└── views/           Pane-content renderers: files · markdown · diff · browser
```

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

## Tests

```bash
make test            # or: cargo test --bin crane
```

Covers the pure Layout tree operations (`split_at`, `remove_node`, `first_leaf`, `collect_leaves`, `contains`, `set_ratio`).

---

## License

[MIT](LICENSE) © rajpootathar
