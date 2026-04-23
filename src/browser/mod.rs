//! Embedded WKWebView management for Browser panes.
//!
//! wry parents a native WKWebView under the main NSWindow's content
//! view. Each Browser pane can hold multiple tabs — we key webviews by
//! `(pane_id, tab_id)` so that switching tabs hides/shows existing
//! webviews (keeping their page state) rather than rebuilding.

pub mod memory;

use crate::state::layout::PaneId;
use parking_lot::Mutex;
use raw_window_handle::HasWindowHandle;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    PageLoadEvent, Rect, WebView, WebViewBuilder,
};
#[cfg(target_os = "macos")]
use wry::WebViewExtMacOS;

/// Shared loading-state map. Each tab key goes in on Started and comes
/// out on Finished. Owned by `BrowserHost`; cloned into the wry page-
/// load callback (which may fire on a background thread — hence the
/// Mutex). Queryable from the egui thread to render spinners.
type LoadingSet = Arc<Mutex<HashSet<SlotKey>>>;

/// Latest URL reported by the WKWebView's navigation callback for each
/// tab. Pushed from a background thread; drained by main.rs each frame
/// and applied back to the tab's state so redirects, link clicks, and
/// history navigation update the URL bar.
type UrlUpdates = Arc<Mutex<HashMap<SlotKey, String>>>;

/// Composite key for the native webview store. Pane id + per-pane tab
/// id. Using a tuple (rather than u64) keeps both sides readable.
pub type SlotKey = (PaneId, u32);

struct Slot {
    webview: WebView,
    last_rect: Option<egui::Rect>,
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
    /// Destroy the webview immediately — used when the owning tab is
    /// closed but the pane itself is still around.
    Close,
}

/// Bridge state populated during `render_layout`:
/// * `alive`   — (key, rect, url) for the **active** tab of each pane.
///               These webviews will be resized + made visible.
/// * `inactive` — (key, url) for other tabs. Webviews are kept around
///                but hidden (so page state survives tab switches).
/// * `actions`  — per-key nav intents (Load / Back / Forward / Reload /
///                Close) queued by clicks this frame.
/// * `focused`  — active-tab slot of the currently-focused Browser
///                pane, if any. Consumed by `sync` to tell mac_keys
///                which WKWebView should receive Cmd+C/V/X/A.
#[derive(Default)]
pub struct Bridge {
    pub alive: Vec<(SlotKey, egui::Rect, String)>,
    pub inactive: Vec<(SlotKey, String)>,
    pub actions: Vec<(SlotKey, Action)>,
    pub focused: Option<SlotKey>,
}

thread_local! {
    pub static BRIDGE: RefCell<Bridge> = RefCell::new(Bridge::default());
    /// Per-frame snapshot of tabs that are currently loading. Written
    /// by `main.rs` at the top of each frame (from
    /// `BrowserHost::loading_set`), read by `browser_view` to render
    /// spinners on the tab chips.
    pub static LOADING_SNAPSHOT: RefCell<HashSet<SlotKey>> =
        RefCell::new(HashSet::new());
}

pub fn set_loading_snapshot(set: HashSet<SlotKey>) {
    LOADING_SNAPSHOT.with(|s| *s.borrow_mut() = set);
}

pub fn is_loading(key: SlotKey) -> bool {
    LOADING_SNAPSHOT.with(|s| s.borrow().contains(&key))
}

thread_local! {
    static MEMORY_SNAPSHOT: RefCell<memory::Snapshot> =
        RefCell::new(memory::Snapshot::default());
}

pub fn set_memory_snapshot(snap: memory::Snapshot) {
    MEMORY_SNAPSHOT.with(|s| *s.borrow_mut() = snap);
}

pub fn memory_snapshot() -> memory::Snapshot {
    MEMORY_SNAPSHOT.with(|s| s.borrow().clone())
}

/// Apply a batch of WKWebView-reported URL changes to the matching
/// BrowserTabs across every workspace's layout. Keeps the egui URL bar
/// in sync with in-page navigation (link clicks, redirects, history).
/// Walk every layout in every workspace/project and collect the
/// `(pane_id, tab_id)` of every Browser tab that exists right now.
/// Used to decide which native webviews may be dropped — panes on
/// inactive workspace tabs still count as "alive" so switching away
/// and back doesn't reload the page.
pub fn collect_all_keys(app: &crate::state::App) -> HashSet<SlotKey> {
    use crate::state::layout::PaneContent;
    let mut out = HashSet::new();
    for project in &app.projects {
        for ws in &project.workspaces {
            for tab in &ws.tabs {
                for (pid, pane) in tab.layout.panes.iter() {
                    if let PaneContent::Browser(bp) = &pane.content {
                        for btab in &bp.tabs {
                            out.insert((*pid, btab.id));
                        }
                    }
                }
            }
        }
    }
    out
}

pub fn apply_url_updates_to_app(
    app: &mut crate::state::App,
    updates: &HashMap<SlotKey, String>,
) {
    use crate::state::layout::PaneContent;
    if updates.is_empty() {
        return;
    }
    for project in &mut app.projects {
        for ws in &mut project.workspaces {
            for tab in &mut ws.tabs {
                for (pid, pane) in tab.layout.panes.iter_mut() {
                    if let PaneContent::Browser(bp) = &mut pane.content {
                        for btab in &mut bp.tabs {
                            let key = (*pid, btab.id);
                            if let Some(new_url) = updates.get(&key) {
                                // Skip updates for the initial blank
                                // placeholder or when WKWebView round-
                                // trips the same URL we just set.
                                if new_url.is_empty() || btab.url == *new_url {
                                    continue;
                                }
                                btab.url = new_url.clone();
                                btab.input_buf = new_url.clone();
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn report_pane(key: SlotKey, rect: egui::Rect, url: &str) {
    BRIDGE.with(|b| {
        b.borrow_mut().alive.push((key, rect, url.to_string()));
    });
}

pub fn report_inactive(key: SlotKey, url: &str) {
    BRIDGE.with(|b| {
        b.borrow_mut().inactive.push((key, url.to_string()));
    });
}

pub fn queue_action(key: SlotKey, action: Action) {
    BRIDGE.with(|b| b.borrow_mut().actions.push((key, action)));
}

/// Report that a Browser pane's active tab currently owns focus.
/// `browser_view::render` calls this when the pane is the focused
/// leaf of its layout AND the pane's native webview is visible this
/// frame (i.e. no modal/overlay is hiding it). `BrowserHost::sync`
/// drains this and passes the slot's WKWebView to mac_keys so
/// Cmd+C/V/X/A can be routed to the embedded browser.
pub fn report_focused_pane(key: SlotKey) {
    BRIDGE.with(|b| b.borrow_mut().focused = Some(key));
}

pub fn take_bridge() -> Bridge {
    BRIDGE.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

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

    /// Snapshot the set of keys whose webviews are currently loading.
    /// Cheap — egui view calls this once per frame.
    pub fn loading_set(&self) -> HashSet<SlotKey> {
        self.loading.lock().clone()
    }

    /// Drain any URL updates the WKWebView navigation callback queued
    /// since the last frame. Main.rs applies these back to the owning
    /// tab's state so the URL bar reflects in-page navigation.
    ///
    /// Also locks in the new URL as each slot's `loaded_url` — critical
    /// for SPA routing: otherwise the next frame's `sync` would see
    /// `tab.url != slot.loaded_url` and issue a `load_url`, which
    /// reloads the page the user just navigated into.
    pub fn drain_url_updates(&mut self) -> HashMap<SlotKey, String> {
        let updates = std::mem::take(&mut *self.url_updates.lock());
        for (key, url) in &updates {
            if let Some(slot) = self.slots.get_mut(key) {
                slot.loaded_url = url.clone();
            }
        }
        updates
    }

    pub fn sync<W: HasWindowHandle>(
        &mut self,
        window: &W,
        ctx: &egui::Context,
        bridge: Bridge,
        hide_all: bool,
        all_keys: &HashSet<SlotKey>,
    ) {
        // A slot belongs to a pane/tab that still exists somewhere in
        // the App (possibly a non-active workspace tab). Only drop
        // webviews whose (pane_id, tab_id) is truly gone — otherwise
        // switching workspace tabs would rebuild the webview every
        // time and reload the page.
        //
        // Before removing, navigate the webview to `about:blank` and
        // flush its caches via a small JS shim. Without this, WKWebView
        // keeps the last page's JS heap, image cache, and service
        // workers alive in the WebContent process even after `drop` —
        // visibly so on heavy sites (Twitter, YouTube), where the
        // "WebKit.WebContent" processes don't shrink after a pane close.
        // Doing it here catches both "pane removed from layout" and
        // "last tab in pane closed" paths.
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
        {
            let mut loading = self.loading.lock();
            loading.retain(|k| all_keys.contains(k));
        }
        // Fold explicit Close actions first so we drop webviews before
        // we'd otherwise try to resize/reload them.
        for (key, action) in bridge.actions {
            if matches!(action, Action::Close) {
                if let Some(slot) = self.slots.get(&key) {
                    release_webview_memory(&slot.webview);
                }
                self.slots.remove(&key);
                self.pending.remove(&key);
                self.loading.lock().remove(&key);
            } else {
                // Eagerly mark the tab as loading the moment a Load is
                // queued. Otherwise the spinner wouldn't appear until
                // WKWebView's async PageLoadEvent::Started fires — on a
                // fast network that's well after "Go" was clicked, so
                // the user never sees feedback.
                if matches!(action, Action::Load(_) | Action::Reload) {
                    self.loading.lock().insert(key);
                    ctx.request_repaint();
                }
                self.pending.entry(key).or_default().push(action);
            }
        }
        let alive = bridge.alive;
        let inactive = bridge.inactive;
        let focused = bridge.focused;

        if hide_all {
            for slot in self.slots.values() {
                let _ = slot.webview.set_visible(false);
            }
            // Webview is hidden — don't route Cmd+C/V/X/A to it.
            // (The overlay — modal, tooltip, popup — owns focus, and
            // its egui TextEdit should receive clipboard events.)
            crate::mac_keys::set_focused_webview(None);
            return;
        }

        // Anything not reported this frame (because its workspace tab
        // isn't active) should be hidden — but NOT destroyed, so
        // returning to it is instant and preserves page state.
        let reported: std::collections::HashSet<SlotKey> = alive
            .iter()
            .map(|(k, _, _)| *k)
            .chain(inactive.iter().map(|(k, _)| *k))
            .collect();
        for (key, slot) in self.slots.iter() {
            if !reported.contains(key) {
                let _ = slot.webview.set_visible(false);
            }
        }

        // Background tabs: keep webview around but hidden, so the page
        // (form state, scroll, audio) sticks around on tab switches.
        for (key, url) in &inactive {
            if !self.slots.contains_key(key) {
                // Build even inactive webviews eagerly so a first-time
                // tab switch is instant — otherwise the initial load
                // wouldn't start until the tab is focused.
                if let Some(slot) = build_slot(
                    window,
                    ctx,
                    egui_placeholder_rect(),
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
                    if slot.last_rect.map(|r| r != *rect).unwrap_or(true) {
                        let _ = slot.webview.set_bounds(wry_rect);
                        slot.last_rect = Some(*rect);
                    }
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
                    if let Some(mut slot) = build_slot(
                        window,
                        ctx,
                        wry_rect,
                        url,
                        *key,
                        self.loading.clone(),
                        self.url_updates.clone(),
                    ) {
                        slot.last_rect = Some(*rect);
                        self.slots.insert(*key, slot);
                    }
                }
            }
        }

        // Tell mac_keys which WKWebView (if any) should receive
        // Cmd+C/V/X/A on the next NSEvent. Done last so we've already
        // (re)built the slot for a newly-focused pane above.
        //
        // We cross an objc2 major-version boundary here: wry 0.55 is
        // built against objc2 0.6 (its `WryWebView` and `Retained`
        // come from that crate version), but mac_keys.rs is built
        // against our direct objc2 0.5 dep. We can't hand a 0.6
        // `Retained` to a 0.5 API, so we pass a raw Obj-C object
        // pointer — retain/release are ABI-stable across versions.
        let focused_view_ptr: Option<std::ptr::NonNull<objc2::runtime::AnyObject>> =
            focused.and_then(|key| {
                let slot = self.slots.get(&key)?;
                // wry's `webview()` returns a clone of its internal
                // `Retained<WryWebView>` (+1 retain). We extract the
                // raw Obj-C pointer; the clone drops at end of arm,
                // but `slot.webview` still holds its own retain so
                // the object stays alive until `set_focused_webview`
                // issues *its* retain below.
                let wryview = slot.webview.webview();
                std::ptr::NonNull::new(
                    &*wryview as *const _ as *mut objc2::runtime::AnyObject,
                )
            });
        crate::mac_keys::set_focused_webview(focused_view_ptr);
    }
}

impl Default for BrowserHost {
    fn default() -> Self {
        Self::new()
    }
}

fn build_slot<W: HasWindowHandle>(
    window: &W,
    ctx: &egui::Context,
    rect: Rect,
    url: &str,
    key: SlotKey,
    loading: LoadingSet,
    url_updates: UrlUpdates,
) -> Option<Slot> {
    // wry's callbacks fire on a background thread. Cloning the egui
    // Context into each one is cheap and lets us wake the UI loop so
    // URL bar updates + spinner state paint immediately instead of
    // waiting for the next user event.
    let ctx_load = ctx.clone();
    let ctx_nav = ctx.clone();
    let ctx_ipc = ctx.clone();
    let builder = WebViewBuilder::new()
        .with_bounds(rect)
        .with_url(if url.is_empty() { "about:blank" } else { url })
        .with_transparent(false)
        // Explicitly off — wry's default is `true` in debug builds
        // (cfg(debug_assertions)), which surfaces "Inspect Element"
        // in the webview's native right-click menu. That entry opens
        // WebKit's _inspector attached to the hosting NSWindow; it
        // covers Crane's panels and has no reachable close affordance.
        // Keep debug and release behaviour identical until we have a
        // proper devtools UX.
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
                ctx_load.request_repaint();
            }
        })
        // Also intercept the pre-navigation callback — catches
        // classic full-page loads and redirect chains.
        .with_navigation_handler({
            let url_updates = url_updates.clone();
            move |url| {
                url_updates.lock().insert(key, url);
                ctx_nav.request_repaint();
                true
            }
        })
        // SPAs (Next.js, React Router, etc.) use history.pushState /
        // replaceState, which WKWebView does NOT surface through the
        // navigation delegate. Inject a tiny script that hooks those
        // methods + popstate and posts the new URL back to us via
        // IPC. "crane-url:" prefix is a cheap tag so we don't confuse
        // other future IPC traffic.
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
                // Initial URL post — useful when the page is loaded at
                // a hash-routed path the nav handler already forgot.
                post();
            })();
            "#,
        )
        .with_ipc_handler(move |req| {
            let body = req.body();
            if let Some(new_url) = body.strip_prefix("crane-url:") {
                url_updates.lock().insert(key, new_url.to_string());
                ctx_ipc.request_repaint();
            }
        });
    match builder.build_as_child(window) {
        Ok(webview) => {
            // Inactive tabs get built hidden; the active-tab pass above
            // will flip them on.
            let _ = webview.set_visible(false);
            Some(Slot {
                webview,
                last_rect: None,
                loaded_url: url.to_string(),
            })
        }
        Err(e) => {
            log::warn!("wry build_as_child failed: {e}");
            None
        }
    }
}

/// Best-effort flush of a webview's in-memory state right before the
/// WebView is dropped. `about:blank` detaches the active document so
/// its JS heap / decoded images / service worker can be reclaimed; the
/// JS clears sessionStorage / localStorage for the origin (for session
/// work — persistent site data is out of scope here). WKWebView still
/// keeps its WebContent process pooled, but the per-tab footprint
/// drops immediately rather than lingering.
fn release_webview_memory(webview: &WebView) {
    // Run cache-clearing JS first so it executes against the live page
    // before navigation wipes the script context. Silent errors are
    // expected (about: / chrome: pages don't expose storage, cross-
    // origin iframes refuse, etc.) — we just try.
    let _ = webview.evaluate_script(
        "try { sessionStorage.clear(); } catch(e) {} \
         try { localStorage.clear(); } catch(e) {} \
         try { if (window.caches) caches.keys().then(ks => ks.forEach(k => caches.delete(k))); } catch(e) {}",
    );
    let _ = webview.load_url("about:blank");
    let _ = webview.set_visible(false);
}

fn egui_placeholder_rect() -> Rect {
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
            // Handled before dispatch — this arm only exists to keep
            // the match exhaustive if it ever leaks through.
        }
    }
}

fn to_wry_rect(r: egui::Rect) -> Rect {
    Rect {
        position: LogicalPosition::new(r.min.x as f64, r.min.y as f64).into(),
        size: LogicalSize::new(r.width() as f64, r.height() as f64).into(),
    }
}
