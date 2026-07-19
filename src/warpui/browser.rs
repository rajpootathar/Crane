//! Embedded WKWebView management for Browser Panes — the warpui port of old
//! Crane's `src/browser/mod.rs` (1:1 where the framework allows).
//!
//! wry parents a native WKWebView under the main NSWindow's content view.
//! Each Browser Pane can hold multiple tabs — webviews are keyed by
//! `(pane_id, tab_id)` so switching tabs hides/shows existing webviews
//! (keeping page state: forms, scroll, auth) rather than rebuilding.
//!
//! Differences from the egui original, forced by the host change:
//! * The per-frame `egui::Context::request_repaint()` wake is replaced by the
//!   shell's `ui_wake` channel (same `Arc<dyn Fn()>` the PTY reader uses).
//! * `sync` is driven by a 33ms shell tick (warpui has no per-frame app hook),
//!   not the end of every painted frame. Bounds are re-applied on every sync —
//!   the "always re-apply, never diff" rule that fixed the DMG stale-rect bug
//!   — so any drift between paint and tick self-heals within a tick.
//! * The mac_keys NSEvent clipboard router does not exist in the warpui
//!   frontend (yet); Cmd+C/V/X/A inside the webview relies on the WKWebView
//!   being the AppKit first responder after a click. Needs runtime
//!   verification; `Bridge::focused` was dropped with it.

pub mod memory {
    //! On-demand poller for total WKWebView memory. Port of the old
    //! `src/browser/memory.rs` — zero threads, zero work unless a Browser
    //! Pane actually calls `snapshot()`; results cached for `POLL_INTERVAL`.

    use parking_lot::Mutex;
    use std::time::{Duration, Instant};

    const POLL_INTERVAL: Duration = Duration::from_secs(3);
    /// Footer chip goes orange at 1 GB…
    pub const WARN_BYTES: u64 = 1_000_000_000;
    /// …and red (with a "close tabs" nudge) at 2 GB.
    pub const DANGER_BYTES: u64 = 2_000_000_000;

    #[derive(Clone, Default)]
    pub struct Snapshot {
        pub total_bytes: u64,
        pub process_count: u32,
    }

    struct Cached {
        snap: Snapshot,
        at: Option<Instant>,
    }

    pub struct Monitor {
        cache: Mutex<Cached>,
    }

    impl Monitor {
        pub fn start() -> Self {
            Self {
                cache: Mutex::new(Cached {
                    snap: Snapshot::default(),
                    at: None,
                }),
            }
        }

        /// Returns the cached value unless `POLL_INTERVAL` has elapsed, in
        /// which case it samples inline (single `ps` invocation, ~5 ms).
        pub fn snapshot(&self) -> Snapshot {
            let mut c = self.cache.lock();
            let stale = c.at.is_none_or(|t| t.elapsed() >= POLL_INTERVAL);
            if stale {
                if let Some(fresh) = sample_webkit_processes() {
                    c.snap = fresh;
                }
                c.at = Some(Instant::now());
            }
            c.snap.clone()
        }
    }

    /// Sum the RSS of every `com.apple.WebKit.WebContent` process. wry doesn't
    /// expose per-webview pids, so per-tab attribution is impossible from here;
    /// the total covers ALL Browser Panes & tabs in Crane.
    fn sample_webkit_processes() -> Option<Snapshot> {
        // `ps -axo rss=,comm=` avoids headers. rss is in KB on macOS.
        let out = std::process::Command::new("ps")
            .args(["-axo", "rss=,comm="])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut bytes = 0u64;
        let mut count = 0u32;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if !trimmed.contains("com.apple.WebKit.WebContent") {
                continue;
            }
            if let Some((rss_str, _)) = trimmed.split_once(char::is_whitespace) {
                if let Ok(rss_kb) = rss_str.parse::<u64>() {
                    bytes += rss_kb * 1024;
                    count += 1;
                }
            }
        }
        Some(Snapshot {
            total_bytes: bytes,
            process_count: count,
        })
    }

    pub fn human_bytes(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;
        if bytes >= GB {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{} MB", bytes / MB)
        } else if bytes >= KB {
            format!("{} KB", bytes / KB)
        } else {
            format!("{bytes} B")
        }
    }
}

use crate::warpui::layout::PaneId;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use warpui::geometry::rect::RectF;
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    PageLoadEvent, Rect, WebView, WebViewBuilder,
};

/// Repaint waker — the shell's `ui_wake` (marks the shell view dirty from any
/// thread). wry's callbacks fire on background threads, so it must be Send.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Shared loading-state map. Each tab key goes in on Started and comes out on
/// Finished; queryable from the UI thread to render tab-chip spinners.
type LoadingSet = Arc<Mutex<HashSet<SlotKey>>>;

/// Latest URL reported by the WKWebView's navigation callbacks per tab. Pushed
/// from background threads; drained by the shell tick and applied back to the
/// owning tab so redirects / link clicks / history nav update the URL bar.
type UrlUpdates = Arc<Mutex<HashMap<SlotKey, String>>>;

/// Composite key for the native webview store: Pane id + per-pane tab id.
pub type SlotKey = (PaneId, u32);

struct Slot {
    webview: WebView,
    loaded_url: String,
}

pub struct BrowserHost {
    slots: HashMap<SlotKey, Slot>,
    pending: HashMap<SlotKey, Vec<Action>>,
    loading: LoadingSet,
    url_updates: UrlUpdates,
    pub memory: memory::Monitor,
}

#[derive(Debug, Clone)]
pub enum Action {
    Load(String),
    Reload,
    Back,
    Forward,
    /// Destroy the webview immediately — used when the owning tab is closed
    /// but the pane itself is still around.
    Close,
}

/// Bridge state populated between syncs:
/// * `alive`    — (key, rect, url) for the **active** tab of each visible
///                pane. These webviews are resized + made visible.
/// * `inactive` — (key, url) for other tabs / panes on non-active Tabs.
///                Webviews are kept but hidden (page state survives).
/// * `actions`  — per-key nav intents queued by clicks since the last sync.
#[derive(Default)]
pub struct Bridge {
    pub alive: Vec<(SlotKey, RectF, String)>,
    pub inactive: Vec<(SlotKey, String)>,
    pub actions: Vec<(SlotKey, Action)>,
}

thread_local! {
    static BRIDGE: RefCell<Bridge> = RefCell::new(Bridge::default());
    /// Snapshot of tabs currently loading, refreshed by the shell tick from
    /// `BrowserHost::loading_set`; read by the view to render spinners.
    static LOADING_SNAPSHOT: RefCell<HashSet<SlotKey>> = RefCell::new(HashSet::new());
    static MEMORY_SNAPSHOT: RefCell<memory::Snapshot> = RefCell::new(memory::Snapshot::default());
}

pub fn set_loading_snapshot(set: HashSet<SlotKey>) {
    LOADING_SNAPSHOT.with(|s| *s.borrow_mut() = set);
}

pub fn is_loading(key: SlotKey) -> bool {
    LOADING_SNAPSHOT.with(|s| s.borrow().contains(&key))
}

pub fn set_memory_snapshot(snap: memory::Snapshot) {
    MEMORY_SNAPSHOT.with(|s| *s.borrow_mut() = snap);
}

pub fn memory_snapshot() -> memory::Snapshot {
    MEMORY_SNAPSHOT.with(|s| s.borrow().clone())
}

/// Queue a nav intent from the Browser view; folded into webview calls on the
/// next shell sync tick. Safe to call from any view code on the UI thread.
pub fn queue_action(key: SlotKey, action: Action) {
    BRIDGE.with(|b| b.borrow_mut().actions.push((key, action)));
}

pub fn take_bridge() -> Bridge {
    BRIDGE.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

// ── Host window handle ───────────────────────────────────────────────────────

/// A `HasWindowHandle` wrapper around the main NSWindow's content view,
/// fetched straight from AppKit. warpui's winit window is private to the
/// framework, but wry only needs the content NSView pointer to parent the
/// WKWebView — and `NSApplication.sharedApplication` is always reachable from
/// the main thread (the shell tick). Retaining is unnecessary: Crane is a
/// single-window app and the window outlives every webview.
#[cfg(target_os = "macos")]
pub struct HostWindow(std::ptr::NonNull<std::ffi::c_void>);

#[cfg(target_os = "macos")]
impl HostWindow {
    /// Content view of Crane's REAL render window, or None while the app is
    /// still starting up. Deterministically selects the visible main/key window
    /// (falling back to the largest visible window) rather than trusting
    /// `NSApp.mainWindow` + `windows.firstObject`: macOS keeps a HIDDEN 500x500
    /// helper window in the app's window list, and when `mainWindow` is
    /// transiently nil (app unfocused, or mid-startup) the old `firstObject`
    /// fallback could hand wry that phantom window — parenting the WKWebView
    /// into a dead, invisible window so pages never render. The old egui
    /// frontend never hit this because eframe held one real window handle;
    /// warpui keeps its window private, so we resolve it ourselves.
    pub fn current() -> Option<Self> {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            if app.is_null() {
                return None;
            }
            let windows: *mut AnyObject = msg_send![app, windows];
            if windows.is_null() {
                return None;
            }
            let count: usize = msg_send![windows, count];
            let mut best: *mut AnyObject = std::ptr::null_mut();
            let mut best_area = 0.0f64;
            for i in 0..count {
                let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                if w.is_null() {
                    continue;
                }
                let visible: bool = msg_send![w, isVisible];
                if !visible {
                    continue; // skips the hidden 500x500 phantom window
                }
                let is_main: bool = msg_send![w, isMainWindow];
                let is_key: bool = msg_send![w, isKeyWindow];
                if is_main || is_key {
                    best = w;
                    break; // the focused window is unambiguously ours
                }
                let f: objc2_foundation::NSRect = msg_send![w, frame];
                let area = f.size.width * f.size.height;
                if area > best_area {
                    best = w;
                    best_area = area;
                }
            }
            if best.is_null() {
                return None;
            }
            let view: *mut AnyObject = msg_send![best, contentView];
            std::ptr::NonNull::new(view as *mut std::ffi::c_void).map(Self)
        }
    }
}

#[cfg(target_os = "macos")]
impl raw_window_handle::HasWindowHandle for HostWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let handle = raw_window_handle::AppKitWindowHandle::new(self.0);
        // SAFETY: the NSView outlives this borrow — it belongs to Crane's one
        // window, which lives for the process; the handle is consumed
        // synchronously by wry inside the same tick.
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(raw_window_handle::RawWindowHandle::AppKit(
                handle,
            ))
        })
    }
}

// ── Host ─────────────────────────────────────────────────────────────────────

impl BrowserHost {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            pending: HashMap::new(),
            loading: Arc::new(Mutex::new(HashSet::new())),
            url_updates: Arc::new(Mutex::new(HashMap::new())),
            memory: memory::Monitor::start(),
        }
    }

    /// True when no Browser Pane has materialised any webview yet — the shell
    /// tick short-circuits on this so an app with no Browser Pane pays nothing.
    pub fn is_idle(&self) -> bool {
        self.slots.is_empty()
    }

    /// Snapshot the set of keys whose webviews are currently loading.
    pub fn loading_set(&self) -> HashSet<SlotKey> {
        self.loading.lock().clone()
    }

    /// Drain URL updates queued by the WKWebView navigation callbacks. Also
    /// locks the new URL in as each slot's `loaded_url` — critical for SPA
    /// routing: otherwise the next sync would see `tab.url != loaded_url` and
    /// issue a `load_url`, reloading the page the user just navigated into.
    pub fn drain_url_updates(&mut self) -> HashMap<SlotKey, String> {
        let updates = std::mem::take(&mut *self.url_updates.lock());
        for (key, url) in &updates {
            if let Some(slot) = self.slots.get_mut(key) {
                slot.loaded_url = url.clone();
            }
        }
        updates
    }

    /// Reconcile native webviews against the bridge: create/resize/show the
    /// active-tab webviews, keep-but-hide inactive ones, drop slots whose
    /// (pane, tab) no longer exists anywhere, and apply queued nav actions.
    #[cfg(target_os = "macos")]
    pub fn sync(
        &mut self,
        window: &HostWindow,
        wake: &Wake,
        bridge: Bridge,
        hide_all: bool,
        all_keys: &HashSet<SlotKey>,
    ) {
        // Drop webviews whose (pane_id, tab_id) is truly gone — panes on
        // non-active Tabs still count as alive so switching Tabs never
        // reloads a page. Flush caches first (about:blank + storage clear)
        // so WKWebView's WebContent process actually shrinks.
        let to_drop: Vec<SlotKey> = self
            .slots
            .keys()
            .copied()
            .filter(|k| !all_keys.contains(k))
            .collect();
        for key in &to_drop {
            if let Some(slot) = self.slots.get(key) {
                release_webview_memory(&slot.webview);
            }
        }
        self.slots.retain(|k, _| all_keys.contains(k));
        self.pending.retain(|k, _| all_keys.contains(k));
        self.loading.lock().retain(|k| all_keys.contains(k));

        // Fold explicit Close actions first so we drop webviews before we'd
        // otherwise try to resize/reload them.
        for (key, action) in bridge.actions {
            if matches!(action, Action::Close) {
                if let Some(slot) = self.slots.get(&key) {
                    release_webview_memory(&slot.webview);
                }
                self.slots.remove(&key);
                self.pending.remove(&key);
                self.loading.lock().remove(&key);
            } else {
                // Eagerly mark the tab loading on Load/Reload so the spinner
                // shows immediately, not when WKWebView's async Started fires.
                if matches!(action, Action::Load(_) | Action::Reload) {
                    self.loading.lock().insert(key);
                    wake();
                }
                self.pending.entry(key).or_default().push(action);
            }
        }
        let alive = bridge.alive;
        let inactive = bridge.inactive;

        if hide_all {
            // WKWebView always paints above the GPU surface — any overlay
            // (modal, context menu, drag preview) would render beneath it.
            // Hide without destroying; page state is kept.
            for slot in self.slots.values() {
                let _ = slot.webview.set_visible(false);
            }
            return;
        }

        // Anything not reported this sync (its Tab isn't active) hides — but
        // is NOT destroyed, so returning is instant with state intact.
        let reported: HashSet<SlotKey> = alive
            .iter()
            .map(|(k, _, _)| *k)
            .chain(inactive.iter().map(|(k, _)| *k))
            .collect();
        for (key, slot) in self.slots.iter() {
            if !reported.contains(key) {
                let _ = slot.webview.set_visible(false);
            }
        }

        // Background tabs: build eagerly (hidden) so a first tab-switch is
        // instant — the initial load starts before the tab is ever focused.
        for (key, url) in &inactive {
            if !self.slots.contains_key(key) {
                if let Some(slot) = build_slot(
                    window,
                    wake,
                    placeholder_rect(),
                    url,
                    *key,
                    self.loading.clone(),
                    self.url_updates.clone(),
                ) {
                    self.slots.insert(*key, slot);
                }
            }
            if let Some(slot) = self.slots.get(key) {
                let _ = slot.webview.set_visible(false);
            }
        }

        for (key, rect, url) in &alive {
            let wry_rect = to_wry_rect(*rect);
            match self.slots.get_mut(key) {
                Some(slot) => {
                    let _ = slot.webview.set_visible(true);
                    // Always re-apply bounds. The old "only on change"
                    // short-circuit left the WKWebView at a stale frame in
                    // DMG-launched builds; setFrame: per sync is cheap.
                    let _ = slot.webview.set_bounds(wry_rect);
                    if slot.loaded_url != *url && !url.is_empty() {
                        let _ = slot.webview.load_url(url);
                        slot.loaded_url = url.clone();
                    }
                    if let Some(actions) = self.pending.remove(key) {
                        for a in actions {
                            apply(&slot.webview, &a);
                            if let Action::Load(u) = &a {
                                slot.loaded_url = u.clone();
                            }
                        }
                    }
                }
                None => {
                    if let Some(slot) = build_slot(
                        window,
                        wake,
                        wry_rect,
                        url,
                        *key,
                        self.loading.clone(),
                        self.url_updates.clone(),
                    ) {
                        self.slots.insert(*key, slot);
                    }
                }
            }
        }
    }
}

impl Default for BrowserHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
fn build_slot(
    window: &HostWindow,
    wake: &Wake,
    rect: Rect,
    url: &str,
    key: SlotKey,
    loading: LoadingSet,
    url_updates: UrlUpdates,
) -> Option<Slot> {
    // wry's callbacks fire on background threads; each clones the shell wake
    // so URL-bar updates + spinner state repaint immediately.
    let wake_load = wake.clone();
    let wake_nav = wake.clone();
    let wake_ipc = wake.clone();
    let builder = WebViewBuilder::new()
        .with_bounds(rect)
        .with_url(if url.is_empty() { "about:blank" } else { url })
        .with_transparent(false)
        // Explicitly off — wry defaults devtools ON in debug builds, which
        // surfaces "Inspect Element" whose inspector covers Crane's panels
        // with no reachable close affordance. Keep debug == release until a
        // proper devtools UX exists.
        .with_devtools(false)
        .with_on_page_load_handler({
            let url_updates = url_updates.clone();
            move |event, url| {
                match event {
                    PageLoadEvent::Started => {
                        loading.lock().insert(key);
                        url_updates.lock().insert(key, url);
                    }
                    PageLoadEvent::Finished => {
                        loading.lock().remove(&key);
                        url_updates.lock().insert(key, url);
                    }
                }
                wake_load();
            }
        })
        // Pre-navigation callback — catches classic full-page loads and
        // redirect chains.
        .with_navigation_handler({
            let url_updates = url_updates.clone();
            move |url| {
                url_updates.lock().insert(key, url);
                wake_nav();
                true
            }
        })
        // SPAs (Next.js, React Router …) use history.pushState/replaceState,
        // which WKWebView does NOT surface through the navigation delegate.
        // Hook those + popstate/hashchange and post the URL back over IPC.
        .with_initialization_script(
            r#"
            (function() {
                if (window.__craneNavHooked) return;
                window.__craneNavHooked = true;
                function post() {
                    try { window.ipc.postMessage("crane-url:" + location.href); } catch (e) {}
                }
                var _push = history.pushState;
                history.pushState = function() { var r = _push.apply(this, arguments); post(); return r; };
                var _replace = history.replaceState;
                history.replaceState = function() { var r = _replace.apply(this, arguments); post(); return r; };
                window.addEventListener('popstate', post);
                window.addEventListener('hashchange', post);
                post();
            })();
            "#,
        )
        .with_ipc_handler(move |req| {
            let body = req.body();
            if let Some(new_url) = body.strip_prefix("crane-url:") {
                url_updates.lock().insert(key, new_url.to_string());
                wake_ipc();
            }
        });
    match builder.build_as_child(window) {
        Ok(webview) => {
            // Built hidden; the active-tab pass flips visibility on.
            let _ = webview.set_visible(false);
            Some(Slot {
                webview,
                loaded_url: url.to_string(),
            })
        }
        Err(e) => {
            eprintln!("crane: wry build_as_child failed: {e}");
            None
        }
    }
}

/// Best-effort flush of a webview's in-memory state right before drop:
/// `about:blank` detaches the document (JS heap / decoded images / service
/// workers reclaimable); the JS clears session/local storage for the origin.
fn release_webview_memory(webview: &WebView) {
    let _ = webview.evaluate_script(
        "try { sessionStorage.clear(); } catch(e) {} \
         try { localStorage.clear(); } catch(e) {} \
         try { if (window.caches) caches.keys().then(ks => ks.forEach(k => caches.delete(k))); } catch(e) {}",
    );
    let _ = webview.load_url("about:blank");
    let _ = webview.set_visible(false);
}

fn placeholder_rect() -> Rect {
    Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(10.0, 10.0).into(),
    }
}

fn apply(webview: &WebView, action: &Action) {
    match action {
        Action::Load(url) if !url.is_empty() => {
            let _ = webview.load_url(url);
        }
        Action::Load(_) => {}
        Action::Reload => {
            let _ = webview.reload();
        }
        Action::Back => {
            let _ = webview.evaluate_script("window.history.back()");
        }
        Action::Forward => {
            let _ = webview.evaluate_script("window.history.forward()");
        }
        Action::Close => {
            // Folded before dispatch — kept for exhaustiveness.
        }
    }
}

fn to_wry_rect(r: RectF) -> Rect {
    Rect {
        position: LogicalPosition::new(r.origin().x() as f64, r.origin().y() as f64).into(),
        size: LogicalSize::new(r.width() as f64, r.height() as f64).into(),
    }
}

// ── URL normalization (ported verbatim from the old browser_view.rs) ────────

pub fn normalize_url(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("about:") {
        return raw.to_string();
    }
    // Loopback / RFC1918 private addresses always use http — a dev server on
    // :3000 has no TLS cert, so https would bounce off the handshake. Also
    // short-circuits the search branch, which used to eat `localhost:3000`.
    if is_local_host(raw) {
        return format!("http://{raw}");
    }
    // Any `host:port` with a numeric non-443 port — http too (dev tunnels,
    // self-hosted services).
    if let Some((_head, tail)) = raw.split_once(':') {
        let port_str: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(port) = port_str.parse::<u16>() {
            if port != 443 {
                return format!("http://{raw}");
            }
        }
    }
    if !raw.contains('.') && !raw.contains('/') {
        return format!("https://duckduckgo.com/?q={}", urlencode(raw));
    }
    format!("https://{raw}")
}

/// Loopback or RFC1918 private-LAN host? Accepts raw input that may carry a
/// trailing `:port` or `/path` — the host segment is split off first.
fn is_local_host(s: &str) -> bool {
    let host_end = s.find(|c: char| c == ':' || c == '/').unwrap_or(s.len());
    let host = &s[..host_end];
    if matches!(host, "localhost" | "0.0.0.0" | "[::1]" | "[::]") {
        return true;
    }
    if host.starts_with("127.") || host.starts_with("192.168.") || host.starts_with("10.") {
        return true;
    }
    // 172.16.0.0 – 172.31.255.255 (RFC1918 middle block).
    if let Some(rest) = host.strip_prefix("172.") {
        let octet: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = octet.parse::<u8>() {
            if (16..=31).contains(&n) {
                return true;
            }
        }
    }
    false
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_explicit_schemes() {
        assert_eq!(normalize_url("https://warp.dev"), "https://warp.dev");
        assert_eq!(normalize_url("http://x.test"), "http://x.test");
        assert_eq!(normalize_url("about:blank"), "about:blank");
    }

    #[test]
    fn normalize_local_hosts_get_http() {
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("127.0.0.1:8080/x"), "http://127.0.0.1:8080/x");
        assert_eq!(normalize_url("192.168.1.5"), "http://192.168.1.5");
        assert_eq!(normalize_url("172.20.0.1"), "http://172.20.0.1");
    }

    #[test]
    fn normalize_non_443_port_is_http() {
        assert_eq!(normalize_url("example.com:8080"), "http://example.com:8080");
    }

    #[test]
    fn normalize_bare_word_searches() {
        assert_eq!(
            normalize_url("rust lifetimes"),
            "https://duckduckgo.com/?q=rust%20lifetimes"
        );
    }

    #[test]
    fn normalize_domain_gets_https() {
        assert_eq!(normalize_url("warp.dev"), "https://warp.dev");
    }

    #[test]
    fn human_bytes_scales() {
        assert_eq!(memory::human_bytes(512), "512 B");
        assert_eq!(memory::human_bytes(2048), "2 KB");
        assert_eq!(memory::human_bytes(5 * 1024 * 1024), "5 MB");
        assert_eq!(memory::human_bytes(2_147_483_648), "2.00 GB");
    }
}
