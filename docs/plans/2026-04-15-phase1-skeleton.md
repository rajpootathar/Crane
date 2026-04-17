# Phase 1: Skeleton — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Get a Go binary that opens a native GPU-rendered window via Rust FFI — the foundation for every subsequent phase.

**Architecture:** Go is the main process entry point. It loads `libcrane_ffi` (a Rust shared library) via CGo and calls `crane_init()`, which creates a winit window with a wgpu surface rendering a solid background color. Rust owns the main thread (winit event loop requirement); Go logic runs on goroutines. The FFI boundary is a C header (`crane.h`) with `extern "C"` Rust exports and Go `//export` callbacks.

**Tech Stack:** Rust 1.94+ (winit, wgpu, serde, serde_json), Go 1.26+ (CGo), Make

**Spec Reference:** `crane/docs/specs/2026-04-15-crane-architecture-design.md` — Phase 1 (Section 12), FFI Boundary (Section 5), Rust Crate Structure (Section 6), Go Module Structure (Section 7), Build System (Section 10)

---

## File Structure

### Rust (crates/)

```
crane/crates/
├── Cargo.toml                          # Workspace manifest
├── crane-proto/
│   ├── Cargo.toml
│   └── src/lib.rs                      # Shared types: PaneId, NodeId, PaneKind, PaneState, Direction, PhysicalRect, LayoutNode, AppLayout
├── crane-layout/
│   ├── Cargo.toml
│   └── src/lib.rs                      # Re-exports crane-proto layout types (empty logic for Phase 1)
├── crane-text/
│   ├── Cargo.toml
│   └── src/lib.rs                      # Empty stub
├── crane-terminal/
│   ├── Cargo.toml
│   └── src/lib.rs                      # Empty stub
├── crane-theme/
│   ├── Cargo.toml
│   └── src/lib.rs                      # Empty stub
├── crane-webview/
│   ├── Cargo.toml
│   └── src/lib.rs                      # Empty stub
├── crane-renderer/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # Renderer struct: wgpu Device, Queue, Surface, frame loop
│       └── clear_pass.rs               # Renders solid background color (proves GPU works)
├── crane-window/
│   ├── Cargo.toml
│   └── src/lib.rs                      # WindowManager: creates winit EventLoop + Window, drives render loop
├── crane-app/
│   ├── Cargo.toml
│   └── src/lib.rs                      # AppHandle: owns WindowManager + Renderer, stores callbacks, processes commands
└── crane-ffi/
    ├── Cargo.toml
    └── src/lib.rs                      # extern "C" exports: crane_init, crane_shutdown, crane_run_event_loop
```

### Go (go/)

```
crane/go/
├── go.mod                              # module crane
├── cmd/crane/
│   └── main.go                         # Entry point: loads libcrane_ffi, registers callbacks, calls crane_init + crane_run_event_loop
└── internal/
    └── ffi/
        ├── bridge.go                   # CGo declarations (#cgo LDFLAGS), Go wrappers for crane_* functions, //export callbacks
        └── types.go                    # Go struct mirrors of crane-proto types (CraneHandle, CraneCallbacks)
```

### Build / Config

```
crane/
├── include/
│   └── crane.h                         # C header: CraneHandle, CraneCallbacks struct, all function declarations
├── Makefile                            # build-rust, build-go, build (both), clean, run
└── .gitignore                          # target/, bin/, *.dylib, *.so, *.dll
```

---

## Task 1: Initialize Rust Workspace + crane-proto

**Files:**
- Create: `crane/crates/Cargo.toml`
- Create: `crane/crates/crane-proto/Cargo.toml`
- Create: `crane/crates/crane-proto/src/lib.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
# crane/crates/Cargo.toml
[workspace]
resolver = "2"
members = [
    "crane-proto",
    "crane-layout",
    "crane-text",
    "crane-terminal",
    "crane-theme",
    "crane-webview",
    "crane-renderer",
    "crane-window",
    "crane-app",
    "crane-ffi",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
log = "0.4"
env_logger = "0.11"
```

- [ ] **Step 2: Create crane-proto Cargo.toml**

```toml
# crane/crates/crane-proto/Cargo.toml
[package]
name = "crane-proto"
version.workspace = true
edition.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 3: Write crane-proto shared types**

```rust
// crane/crates/crane-proto/src/lib.rs

use serde::{Deserialize, Serialize};

/// Unique identifier for a pane within the layout tree.
pub type PaneId = u32;

/// Unique identifier for a layout node (split container or group).
pub type NodeId = u32;

/// The kind of content a pane displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    Terminal,
    Editor,
    Browser,
    Diff,
    FileExplorer,
    Markdown,
    Image,
}

/// Lifecycle state of a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneState {
    Ghost,
    Active,
    Suspended,
}

/// Split direction for layout nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Horizontal,
    Vertical,
}

/// A rectangle in physical pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhysicalRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Describes a single pane within a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneDescriptor {
    pub id: PaneId,
    pub kind: PaneKind,
    pub state: PaneState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A node in the layout tree — either a split container or a leaf group.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Split {
        id: NodeId,
        direction: Direction,
        sizes: Vec<f32>,
        children: Vec<LayoutNode>,
    },
    Group {
        id: NodeId,
        #[serde(skip_serializing_if = "Option::is_none")]
        active_pane: Option<PaneId>,
        panes: Vec<PaneDescriptor>,
    },
}

/// The complete application layout sent via crane_apply_layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLayout {
    pub center: LayoutNode,
}

/// Opaque handle to the Crane application instance.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CraneHandle {
    pub id: u64,
}
```

- [ ] **Step 4: Verify crane-proto compiles**

Run from `crane/crates/`:
```bash
cargo check -p crane-proto
```
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/crates/Cargo.toml crane/crates/crane-proto/
git commit -m "feat(crane): initialize Rust workspace and crane-proto shared types"
```

---

## Task 2: Create Stub Crates (layout, text, terminal, theme, webview)

**Files:**
- Create: `crane/crates/crane-layout/Cargo.toml`
- Create: `crane/crates/crane-layout/src/lib.rs`
- Create: `crane/crates/crane-text/Cargo.toml`
- Create: `crane/crates/crane-text/src/lib.rs`
- Create: `crane/crates/crane-terminal/Cargo.toml`
- Create: `crane/crates/crane-terminal/src/lib.rs`
- Create: `crane/crates/crane-theme/Cargo.toml`
- Create: `crane/crates/crane-theme/src/lib.rs`
- Create: `crane/crates/crane-webview/Cargo.toml`
- Create: `crane/crates/crane-webview/src/lib.rs`

- [ ] **Step 1: Create crane-layout**

```toml
# crane/crates/crane-layout/Cargo.toml
[package]
name = "crane-layout"
version.workspace = true
edition.workspace = true

[dependencies]
crane-proto = { path = "../crane-proto" }
```

```rust
// crane/crates/crane-layout/src/lib.rs

// Re-export proto types used by layout consumers.
pub use crane_proto::{
    AppLayout, Direction, LayoutNode, NodeId, PaneDescriptor, PaneId, PaneKind, PaneState,
    PhysicalRect,
};

/// Compute pixel rectangles for each pane in the layout tree.
/// Phase 3 will implement the actual recursive algorithm.
pub fn compute_rects(_node: &LayoutNode, _available: PhysicalRect) -> Vec<(PaneId, PhysicalRect)> {
    Vec::new()
}
```

- [ ] **Step 2: Create crane-text (stub)**

```toml
# crane/crates/crane-text/Cargo.toml
[package]
name = "crane-text"
version.workspace = true
edition.workspace = true
```

```rust
// crane/crates/crane-text/src/lib.rs

// Phase 2: font loading, glyph atlas, cell metrics via cosmic-text.
```

- [ ] **Step 3: Create crane-terminal (stub)**

```toml
# crane/crates/crane-terminal/Cargo.toml
[package]
name = "crane-terminal"
version.workspace = true
edition.workspace = true
```

```rust
// crane/crates/crane-terminal/src/lib.rs

// Phase 2: alacritty_terminal integration, per-thread VT parsing.
```

- [ ] **Step 4: Create crane-theme (stub)**

```toml
# crane/crates/crane-theme/Cargo.toml
[package]
name = "crane-theme"
version.workspace = true
edition.workspace = true
```

```rust
// crane/crates/crane-theme/src/lib.rs

// Phase 7: YAML theme parsing + hot-reload.
```

- [ ] **Step 5: Create crane-webview (stub)**

```toml
# crane/crates/crane-webview/Cargo.toml
[package]
name = "crane-webview"
version.workspace = true
edition.workspace = true
```

```rust
// crane/crates/crane-webview/src/lib.rs

// Phase 6: WKWebView / WebView2 / WebKitGTK platform wrappers.
```

- [ ] **Step 6: Verify all stubs compile**

Run from `crane/crates/`:
```bash
cargo check --workspace
```
Expected: all 6 crates compile (proto + 5 stubs).

- [ ] **Step 7: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/crates/crane-layout/ crane/crates/crane-text/ crane/crates/crane-terminal/ crane/crates/crane-theme/ crane/crates/crane-webview/
git commit -m "feat(crane): add stub crates for layout, text, terminal, theme, webview"
```

---

## Task 3: Implement crane-renderer (wgpu clear pass)

**Files:**
- Create: `crane/crates/crane-renderer/Cargo.toml`
- Create: `crane/crates/crane-renderer/src/lib.rs`
- Create: `crane/crates/crane-renderer/src/clear_pass.rs`

- [ ] **Step 1: Create crane-renderer Cargo.toml**

```toml
# crane/crates/crane-renderer/Cargo.toml
[package]
name = "crane-renderer"
version.workspace = true
edition.workspace = true

[dependencies]
crane-proto = { path = "../crane-proto" }
wgpu = "25"
log = { workspace = true }
```

- [ ] **Step 2: Write the clear pass module**

```rust
// crane/crates/crane-renderer/src/clear_pass.rs

use wgpu::Color;

/// Background color: Crane dark navy (#0e1018).
pub const BACKGROUND_COLOR: Color = Color {
    r: 0.055,
    g: 0.063,
    b: 0.094,
    a: 1.0,
};

/// Execute a render pass that clears the surface to the background color.
pub fn render_clear_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(BACKGROUND_COLOR),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
}
```

- [ ] **Step 3: Write the Renderer struct**

```rust
// crane/crates/crane-renderer/src/lib.rs

mod clear_pass;

use std::sync::Arc;

/// Owns the wgpu device, queue, and surface. Renders frames.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
}

impl Renderer {
    /// Create a new Renderer attached to the given window.
    pub async fn new(window: Arc<winit::window::Window>) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("failed to find a suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("crane_device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            }, None)
            .await
            .expect("failed to create wgpu device");

        let size = window.inner_size();
        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        Self {
            device,
            queue,
            surface,
            surface_config,
        }
    }

    /// Handle window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
        }
    }

    /// Render one frame (currently just a clear pass).
    pub fn render_frame(&self) {
        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            Err(e) => {
                log::error!("surface error: {e}");
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("crane_encoder"),
            });

        clear_pass::render_clear_pass(&mut encoder, &view);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}
```

- [ ] **Step 4: Verify crane-renderer compiles**

Run from `crane/crates/`:
```bash
cargo check -p crane-renderer
```
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/crates/crane-renderer/
git commit -m "feat(crane): implement crane-renderer with wgpu clear pass"
```

---

## Task 4: Implement crane-window (winit event loop)

**Files:**
- Create: `crane/crates/crane-window/Cargo.toml`
- Create: `crane/crates/crane-window/src/lib.rs`

- [ ] **Step 1: Create crane-window Cargo.toml**

```toml
# crane/crates/crane-window/Cargo.toml
[package]
name = "crane-window"
version.workspace = true
edition.workspace = true

[dependencies]
crane-proto = { path = "../crane-proto" }
crane-renderer = { path = "../crane-renderer" }
winit = "0.31"
log = { workspace = true }
```

- [ ] **Step 2: Write the WindowManager**

```rust
// crane/crates/crane-window/src/lib.rs

use std::sync::Arc;

use crane_renderer::Renderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

/// Callback function pointers from Go, stored as raw function pointers.
/// In Phase 1, only on_window_close is wired.
#[derive(Clone)]
pub struct WindowCallbacks {
    pub on_window_close: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Default for WindowCallbacks {
    fn default() -> Self {
        Self {
            on_window_close: None,
        }
    }
}

/// Manages the OS window and drives the winit event loop.
pub struct WindowManager {
    callbacks: WindowCallbacks,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
}

impl WindowManager {
    pub fn new(callbacks: WindowCallbacks) -> Self {
        Self {
            callbacks,
            window: None,
            renderer: None,
        }
    }

    /// Run the event loop. This blocks the calling thread forever (winit requirement).
    pub fn run(mut self) {
        let event_loop = EventLoop::new().expect("failed to create event loop");
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        event_loop.run_app(&mut self).expect("event loop error");
    }
}

impl ApplicationHandler for WindowManager {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("Crane")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));

        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));

        let renderer = pollster::block_on(Renderer::new(window.clone()));

        self.window = Some(window);
        self.renderer = Some(renderer);

        log::info!("Crane window created");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(ref cb) = self.callbacks.on_window_close {
                    cb();
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(ref mut renderer) = self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(ref renderer) = self.renderer {
                    renderer.render_frame();
                }
                if let Some(ref window) = self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 3: Add pollster dependency to workspace**

Add to `crane/crates/Cargo.toml` under `[workspace.dependencies]`:

```toml
pollster = "0.4"
winit = "0.31"
wgpu = "25"
```

Update `crane/crates/crane-window/Cargo.toml`:

```toml
[dependencies]
crane-proto = { path = "../crane-proto" }
crane-renderer = { path = "../crane-renderer" }
winit = { workspace = true }
pollster = { workspace = true }
log = { workspace = true }
```

Update `crane/crates/crane-renderer/Cargo.toml`:

```toml
[dependencies]
crane-proto = { path = "../crane-proto" }
wgpu = { workspace = true }
winit = { workspace = true }
log = { workspace = true }
```

- [ ] **Step 4: Verify crane-window compiles**

Run from `crane/crates/`:
```bash
cargo check -p crane-window
```
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/crates/crane-window/ crane/crates/Cargo.toml crane/crates/crane-renderer/Cargo.toml
git commit -m "feat(crane): implement crane-window with winit event loop and resize handling"
```

---

## Task 5: Implement crane-app (AppHandle coordinator)

**Files:**
- Create: `crane/crates/crane-app/Cargo.toml`
- Create: `crane/crates/crane-app/src/lib.rs`

- [ ] **Step 1: Create crane-app Cargo.toml**

```toml
# crane/crates/crane-app/Cargo.toml
[package]
name = "crane-app"
version.workspace = true
edition.workspace = true

[dependencies]
crane-proto = { path = "../crane-proto" }
crane-window = { path = "../crane-window" }
log = { workspace = true }
```

- [ ] **Step 2: Write the AppHandle**

```rust
// crane/crates/crane-app/src/lib.rs

use std::ffi::c_void;

use crane_proto::CraneHandle;
use crane_window::{WindowCallbacks, WindowManager};

/// Raw C function pointer types matching crane.h CraneCallbacks.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawCallbacks {
    pub on_input: Option<unsafe extern "C" fn(u32, *const u8, usize, *mut c_void)>,
    pub on_pane_focus: Option<unsafe extern "C" fn(u32, *mut c_void)>,
    pub on_pane_resize: Option<unsafe extern "C" fn(u32, u16, u16, *mut c_void)>,
    pub on_layout_action: Option<unsafe extern "C" fn(*const i8, *mut c_void)>,
    pub on_window_event: Option<unsafe extern "C" fn(*const i8, *mut c_void)>,
    pub ctx: *mut c_void,
}

// RawCallbacks contains raw pointers which are safe to send across threads
// because Go guarantees the ctx pointer outlives the Rust side.
unsafe impl Send for RawCallbacks {}
unsafe impl Sync for RawCallbacks {}

/// The top-level application coordinator.
/// Phase 1: owns callbacks and launches the window.
pub struct AppHandle {
    callbacks: RawCallbacks,
}

impl AppHandle {
    pub fn new(callbacks: RawCallbacks) -> Self {
        Self { callbacks }
    }

    /// Start the application. This blocks the calling thread (winit event loop).
    pub fn run(self) -> CraneHandle {
        let callbacks = self.callbacks;

        let window_callbacks = WindowCallbacks {
            on_window_close: Some(Box::new(move || {
                if let Some(on_event) = callbacks.on_window_event {
                    let msg = b"{\"type\":\"close_requested\"}\0";
                    unsafe {
                        on_event(msg.as_ptr() as *const i8, callbacks.ctx);
                    }
                }
            })),
        };

        let handle = CraneHandle { id: 1 };
        let wm = WindowManager::new(window_callbacks);
        wm.run();

        handle
    }
}
```

- [ ] **Step 3: Verify crane-app compiles**

Run from `crane/crates/`:
```bash
cargo check -p crane-app
```
Expected: compiles with no errors.

- [ ] **Step 4: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/crates/crane-app/
git commit -m "feat(crane): implement crane-app AppHandle with callback registration"
```

---

## Task 6: Implement crane-ffi (C-ABI exports)

**Files:**
- Create: `crane/crates/crane-ffi/Cargo.toml`
- Create: `crane/crates/crane-ffi/src/lib.rs`

- [ ] **Step 1: Create crane-ffi Cargo.toml**

```toml
# crane/crates/crane-ffi/Cargo.toml
[package]
name = "crane-ffi"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
crane-proto = { path = "../crane-proto" }
crane-app = { path = "../crane-app" }
log = { workspace = true }
env_logger = { workspace = true }
```

- [ ] **Step 2: Write the FFI exports**

```rust
// crane/crates/crane-ffi/src/lib.rs

use std::ffi::c_void;
use std::panic;

use crane_app::{AppHandle, RawCallbacks};
use crane_proto::CraneHandle;

/// C-compatible callbacks struct matching crane.h.
#[repr(C)]
pub struct CraneCallbacks {
    pub on_input: Option<unsafe extern "C" fn(u32, *const u8, usize, *mut c_void)>,
    pub on_pane_focus: Option<unsafe extern "C" fn(u32, *mut c_void)>,
    pub on_pane_resize: Option<unsafe extern "C" fn(u32, u16, u16, *mut c_void)>,
    pub on_layout_action: Option<unsafe extern "C" fn(*const i8, *mut c_void)>,
    pub on_window_event: Option<unsafe extern "C" fn(*const i8, *mut c_void)>,
    pub ctx: *mut c_void,
}

/// Initialize and run Crane. Blocks the calling thread (winit event loop).
///
/// # Safety
/// `config_json` must be a valid null-terminated UTF-8 string or null.
/// `callbacks` function pointers and ctx must remain valid for the app lifetime.
#[no_mangle]
pub unsafe extern "C" fn crane_init(
    _config_json: *const i8,
    callbacks: CraneCallbacks,
) -> CraneHandle {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("crane_init called");

    let raw = RawCallbacks {
        on_input: callbacks.on_input,
        on_pane_focus: callbacks.on_pane_focus,
        on_pane_resize: callbacks.on_pane_resize,
        on_layout_action: callbacks.on_layout_action,
        on_window_event: callbacks.on_window_event,
        ctx: callbacks.ctx,
    };

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let app = AppHandle::new(raw);
        app.run()
    }));

    match result {
        Ok(handle) => handle,
        Err(_) => {
            log::error!("crane_init panicked");
            CraneHandle { id: 0 }
        }
    }
}

/// Shut down Crane and release resources.
#[no_mangle]
pub extern "C" fn crane_shutdown(_handle: CraneHandle) {
    log::info!("crane_shutdown called");
}
```

- [ ] **Step 3: Verify crane-ffi compiles as cdylib**

Run from `crane/crates/`:
```bash
cargo build -p crane-ffi
```
Expected: produces `target/debug/libcrane_ffi.dylib` (macOS) or `target/debug/libcrane_ffi.so` (Linux).

- [ ] **Step 4: Verify the shared library exports the symbols**

```bash
nm -gU crane/crates/target/debug/libcrane_ffi.dylib | grep crane_
```
Expected output includes:
```
... T _crane_init
... T _crane_shutdown
```

- [ ] **Step 5: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/crates/crane-ffi/
git commit -m "feat(crane): implement crane-ffi with C-ABI exports for crane_init and crane_shutdown"
```

---

## Task 7: Create the C Header (crane.h)

**Files:**
- Create: `crane/include/crane.h`

- [ ] **Step 1: Write the C header**

```c
/* crane/include/crane.h
 *
 * C-ABI interface between Go (consumer) and Rust (provider).
 * This header defines the full FFI surface for the Crane application.
 */

#ifndef CRANE_H
#define CRANE_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* --- Types --- */

typedef struct {
    uint64_t id;
} CraneHandle;

typedef struct {
    void (*on_input)(uint32_t pane_id, const uint8_t *data, size_t len, void *ctx);
    void (*on_pane_focus)(uint32_t pane_id, void *ctx);
    void (*on_pane_resize)(uint32_t pane_id, uint16_t cols, uint16_t rows, void *ctx);
    void (*on_layout_action)(const char *action_json, void *ctx);
    void (*on_window_event)(const char *event_json, void *ctx);
    void *ctx;
} CraneCallbacks;

/* --- Layer 1: Init / Teardown --- */

CraneHandle crane_init(const char *config_json, CraneCallbacks callbacks);
void crane_shutdown(CraneHandle handle);
bool crane_set_theme(CraneHandle handle, const char *theme_json);

/* --- Layer 2: Commands (Go -> Rust) --- */

bool crane_apply_layout(CraneHandle handle, const char *layout_json);
bool crane_write_terminal(CraneHandle handle, uint32_t pane_id, const uint8_t *data, size_t len);
bool crane_resize_pane(CraneHandle handle, uint32_t pane_id, uint16_t cols, uint16_t rows);
bool crane_set_pane_state(CraneHandle handle, uint32_t pane_id, uint8_t state);
bool crane_set_breadcrumb(CraneHandle handle, const char *text_json);
bool crane_open_url(CraneHandle handle, uint32_t pane_id, const char *url);
const char *crane_query_font_metrics(CraneHandle handle, const char *font_name, float size);
void crane_set_ime_state(CraneHandle handle, uint32_t pane_id, bool composing);
uint8_t *crane_capture_screenshot(CraneHandle handle, uint32_t pane_id, size_t *out_len);
uint8_t *crane_capture_scrollback(CraneHandle handle, uint32_t pane_id, size_t *out_len);
bool crane_replay_scrollback(CraneHandle handle, uint32_t pane_id, const uint8_t *data, size_t len);
void crane_free_bytes(uint8_t *ptr, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* CRANE_H */
```

- [ ] **Step 2: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/include/
git commit -m "feat(crane): add crane.h C header defining full FFI surface"
```

---

## Task 8: Initialize Go Module + FFI Bridge

**Files:**
- Create: `crane/go/go.mod`
- Create: `crane/go/internal/ffi/types.go`
- Create: `crane/go/internal/ffi/bridge.go`

- [ ] **Step 1: Initialize Go module**

```bash
cd /Users/rajpootathar/ideaProjects/superset/crane/go
go mod init crane
```

- [ ] **Step 2: Write FFI types**

```go
// crane/go/internal/ffi/types.go

package ffi

// #include "../../include/crane.h"
import "C"

// CraneHandle wraps the opaque handle from Rust.
type CraneHandle struct {
	raw C.CraneHandle
}

// Valid returns true if the handle was initialized successfully.
func (h CraneHandle) Valid() bool {
	return h.raw.id != 0
}
```

- [ ] **Step 3: Write FFI bridge with CGo bindings and callback exports**

```go
// crane/go/internal/ffi/bridge.go

package ffi

/*
#cgo CFLAGS: -I${SRCDIR}/../../include
#cgo LDFLAGS: -L${SRCDIR}/../../crates/target/debug -lcrane_ffi
#cgo darwin LDFLAGS: -Wl,-rpath,@executable_path/../lib -Wl,-rpath,${SRCDIR}/../../crates/target/debug
#cgo linux LDFLAGS: -Wl,-rpath,$ORIGIN/../lib -Wl,-rpath,${SRCDIR}/../../crates/target/debug
#include "crane.h"
#include <stdlib.h>

// Forward declarations for Go callbacks.
extern void goOnInput(unsigned int pane_id, const unsigned char *data, size_t len, void *ctx);
extern void goOnPaneFocus(unsigned int pane_id, void *ctx);
extern void goOnPaneResize(unsigned int pane_id, unsigned short cols, unsigned short rows, void *ctx);
extern void goOnLayoutAction(const char *action_json, void *ctx);
extern void goOnWindowEvent(const char *event_json, void *ctx);
*/
import "C"

import (
	"log"
	"unsafe"
)

// --- Callbacks from Rust → Go ---

// These are registered with Rust during crane_init.
// They run on Rust-allocated threads. Keep them short — just dispatch to channels.

//export goOnInput
func goOnInput(paneID C.uint, data *C.uchar, length C.size_t, ctx unsafe.Pointer) {
	// Phase 2: dispatch to PTY write queue
	_ = paneID
	_ = data
	_ = length
}

//export goOnPaneFocus
func goOnPaneFocus(paneID C.uint, ctx unsafe.Pointer) {
	// Phase 3: dispatch to app event loop
	_ = paneID
}

//export goOnPaneResize
func goOnPaneResize(paneID C.uint, cols C.ushort, rows C.ushort, ctx unsafe.Pointer) {
	// Phase 3: dispatch resize
	_ = paneID
	_ = cols
	_ = rows
}

//export goOnLayoutAction
func goOnLayoutAction(actionJSON *C.char, ctx unsafe.Pointer) {
	// Phase 3: dispatch layout action
	_ = actionJSON
}

//export goOnWindowEvent
func goOnWindowEvent(eventJSON *C.char, ctx unsafe.Pointer) {
	event := C.GoString(eventJSON)
	log.Printf("[crane] window event: %s", event)
}

// --- Go → Rust commands ---

// Init initializes the Crane application. Blocks the calling thread (winit event loop).
func Init(configJSON string) CraneHandle {
	var cConfig *C.char
	if configJSON != "" {
		cConfig = C.CString(configJSON)
		defer C.free(unsafe.Pointer(cConfig))
	}

	callbacks := C.CraneCallbacks{
		on_input:         C.CraneCallbacks_on_input_func(C.goOnInput),
		on_pane_focus:    C.CraneCallbacks_on_pane_focus_func(C.goOnPaneFocus),
		on_pane_resize:   C.CraneCallbacks_on_pane_resize_func(C.goOnPaneResize),
		on_layout_action: C.CraneCallbacks_on_layout_action_func(C.goOnLayoutAction),
		on_window_event:  C.CraneCallbacks_on_window_event_func(C.goOnWindowEvent),
		ctx:              nil,
	}

	handle := C.crane_init(cConfig, callbacks)
	return CraneHandle{raw: handle}
}

// Shutdown shuts down Crane and releases resources.
func Shutdown(h CraneHandle) {
	C.crane_shutdown(h.raw)
}
```

- [ ] **Step 4: Check that the CGo bindings parse correctly**

CGo's function pointer casting syntax can be tricky. If the `CraneCallbacks_on_input_func` style doesn't work with your CGo version, replace the callback assignment block with direct unsafe casts:

```go
	callbacks := C.CraneCallbacks{}
	callbacks.on_input = (*[0]byte)(C.goOnInput)
	callbacks.on_pane_focus = (*[0]byte)(C.goOnPaneFocus)
	callbacks.on_pane_resize = (*[0]byte)(C.goOnPaneResize)
	callbacks.on_layout_action = (*[0]byte)(C.goOnLayoutAction)
	callbacks.on_window_event = (*[0]byte)(C.goOnWindowEvent)
	callbacks.ctx = nil
```

This is the standard CGo pattern for assigning Go-exported functions to C function pointer fields.

- [ ] **Step 5: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/go/
git commit -m "feat(crane): initialize Go module with CGo FFI bridge and callback exports"
```

---

## Task 9: Write Go main.go Entry Point

**Files:**
- Create: `crane/go/cmd/crane/main.go`

- [ ] **Step 1: Write main.go**

```go
// crane/go/cmd/crane/main.go

package main

import (
	"log"
	"runtime"

	"crane/internal/ffi"
)

func init() {
	// winit requires the main thread on macOS.
	// LockOSThread pins this goroutine to the main OS thread.
	runtime.LockOSThread()
}

func main() {
	log.Println("[crane] starting Crane...")

	// crane_init blocks (runs the winit event loop on this thread).
	// Go goroutines run on other OS threads managed by the Go runtime.
	handle := ffi.Init("{}")

	if !handle.Valid() {
		log.Fatal("[crane] failed to initialize — crane_init returned invalid handle")
	}

	// We only reach here after the window is closed.
	log.Println("[crane] window closed, shutting down...")
	ffi.Shutdown(handle)
	log.Println("[crane] done")
}
```

- [ ] **Step 2: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/go/cmd/
git commit -m "feat(crane): add Go main entry point with runtime.LockOSThread for winit"
```

---

## Task 10: Create Makefile + .gitignore

**Files:**
- Create: `crane/Makefile`
- Create: `crane/.gitignore`

- [ ] **Step 1: Write the Makefile**

```makefile
# crane/Makefile

CRATES_DIR := crates
GO_DIR := go
BIN_DIR := bin
INCLUDE_DIR := include

# Detect OS for library extension
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
    LIB_EXT := dylib
    LIB_PREFIX := lib
else ifeq ($(UNAME_S),Linux)
    LIB_EXT := so
    LIB_PREFIX := lib
else
    LIB_EXT := dll
    LIB_PREFIX :=
endif

LIB_NAME := $(LIB_PREFIX)crane_ffi.$(LIB_EXT)
RUST_TARGET_DIR := $(CRATES_DIR)/target

.PHONY: build build-rust build-rust-debug build-go run clean check

# Default: debug build
build: build-rust-debug build-go

# Release build
release: build-rust build-go-release

build-rust:
	cd $(CRATES_DIR) && cargo build --release -p crane-ffi

build-rust-debug:
	cd $(CRATES_DIR) && cargo build -p crane-ffi

build-go:
	@mkdir -p $(BIN_DIR)
	cd $(GO_DIR) && \
		CGO_ENABLED=1 \
		CGO_CFLAGS="-I$(CURDIR)/$(INCLUDE_DIR)" \
		CGO_LDFLAGS="-L$(CURDIR)/$(RUST_TARGET_DIR)/debug -lcrane_ffi" \
		go build -o $(CURDIR)/$(BIN_DIR)/crane ./cmd/crane

build-go-release:
	@mkdir -p $(BIN_DIR)
	cd $(GO_DIR) && \
		CGO_ENABLED=1 \
		CGO_CFLAGS="-I$(CURDIR)/$(INCLUDE_DIR)" \
		CGO_LDFLAGS="-L$(CURDIR)/$(RUST_TARGET_DIR)/release -lcrane_ffi" \
		go build -o $(CURDIR)/$(BIN_DIR)/crane ./cmd/crane

run: build
	cd $(BIN_DIR) && ./crane

check:
	cd $(CRATES_DIR) && cargo check --workspace
	cd $(GO_DIR) && CGO_ENABLED=1 CGO_CFLAGS="-I$(CURDIR)/$(INCLUDE_DIR)" go vet ./...

clean:
	cd $(CRATES_DIR) && cargo clean
	rm -rf $(BIN_DIR)

test-rust:
	cd $(CRATES_DIR) && cargo test --workspace

lint:
	cd $(CRATES_DIR) && cargo clippy --workspace -- -D warnings
```

- [ ] **Step 2: Write .gitignore**

```gitignore
# crane/.gitignore

# Rust build artifacts
crates/target/

# Go binary output
bin/

# Shared libraries (built artifacts, not source)
*.dylib
*.so
*.dll

# OS files
.DS_Store
```

- [ ] **Step 3: Commit**

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/Makefile crane/.gitignore
git commit -m "feat(crane): add Makefile with build/run/clean targets and .gitignore"
```

---

## Task 11: Build and Run — End-to-End Verification

**Files:** No new files — this task verifies everything works together.

- [ ] **Step 1: Build Rust shared library**

```bash
cd /Users/rajpootathar/ideaProjects/superset/crane
make build-rust-debug
```
Expected: `crates/target/debug/libcrane_ffi.dylib` exists.

- [ ] **Step 2: Verify symbols are exported**

```bash
nm -gU crates/target/debug/libcrane_ffi.dylib | grep crane_
```
Expected output includes `_crane_init` and `_crane_shutdown`.

- [ ] **Step 3: Build Go binary**

```bash
cd /Users/rajpootathar/ideaProjects/superset/crane
make build-go
```
Expected: `bin/crane` binary exists.

- [ ] **Step 4: Run Crane**

```bash
cd /Users/rajpootathar/ideaProjects/superset/crane
make run
```
Expected:
1. Log output: `[crane] starting Crane...`
2. A native window opens with title "Crane" (1280x800)
3. The window background is dark navy (#0e1018)
4. The window is resizable
5. Closing the window prints: `[crane] window event: {"type":"close_requested"}`
6. Process exits cleanly with: `[crane] done`

- [ ] **Step 5: Fix any issues**

If the Go build fails with "undefined reference to crane_init", verify:
- `libcrane_ffi.dylib` exists in `crates/target/debug/`
- The CGo LDFLAGS path in `bridge.go` resolves correctly
- On macOS, the rpath is set via `-Wl,-rpath`

If the window doesn't appear, verify:
- `runtime.LockOSThread()` is called in `init()` (winit needs the main thread on macOS)
- The wgpu adapter was found (check log output for "failed to find a suitable GPU adapter")

- [ ] **Step 6: Run the full build via make**

```bash
cd /Users/rajpootathar/ideaProjects/superset/crane
make clean && make build && make run
```
Expected: clean build from scratch, window opens, closes cleanly.

- [ ] **Step 7: Commit any fixes**

If any changes were needed during verification:

```bash
cd /Users/rajpootathar/ideaProjects/superset
git add crane/
git commit -m "fix(crane): resolve build issues from end-to-end verification"
```

---

## Summary

| Task | What it produces |
|------|-----------------|
| 1 | Rust workspace + crane-proto shared types |
| 2 | 5 stub crates (layout, text, terminal, theme, webview) |
| 3 | crane-renderer with wgpu clear pass |
| 4 | crane-window with winit event loop |
| 5 | crane-app AppHandle coordinator |
| 6 | crane-ffi C-ABI exports (cdylib) |
| 7 | crane.h C header |
| 8 | Go module + CGo FFI bridge |
| 9 | Go main.go entry point |
| 10 | Makefile + .gitignore |
| 11 | End-to-end build and run verification |

**Phase 1 deliverable:** `make run` opens a native GPU-rendered window from a Go binary, with the Rust rendering library loaded via FFI. The window displays a dark navy background, handles resize, and exits cleanly when closed.
