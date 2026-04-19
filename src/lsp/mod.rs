//! LSP client — diagnostics + hover for Rust, TS/JS/TSX/JSX, Go, Python, CSS, HTML.
//!
//! One server process per language, spawned lazily on first file open.
//! Communication is JSON-RPC over stdio (see `protocol.rs`). Server messages
//! are parsed on a background thread and written to a shared state struct;
//! the UI polls that state each repaint.

pub mod downloader;
pub mod protocol;
pub mod server;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use downloader::{DownloadState, Downloader};
pub use server::{Diagnostic, Location, ServerKey};

/// Per-language behavior toggles. Persisted in the session so users don't
/// have to reconfigure on every launch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanguageConfig {
    /// If false, don't spawn the LSP server or request diagnostics for
    /// this language at all.
    pub enabled: bool,
    /// Send `textDocument/didSave` on save. rust-analyzer uses this to
    /// trigger `cargo check` (full compile error coverage). Off for
    /// languages that don't need an on-save checker.
    pub check_on_save: bool,
    /// Run the server's formatter (`textDocument/formatting`) on save and
    /// apply returned TextEdits. Requires hover-style response handling;
    /// wired up in Phase 2.
    pub format_on_save: bool,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_on_save: true,
            format_on_save: false,
        }
    }
}

impl LanguageConfig {
    /// Sensible defaults per server. Rust gets `check_on_save` because
    /// that's where the real errors come from. JS/TS/Python live-type
    /// check so they don't need it as much but it's still a useful nudge.
    pub fn defaults_for(key: ServerKey) -> Self {
        match key {
            ServerKey::RustAnalyzer => Self {
                enabled: true,
                check_on_save: true,
                format_on_save: false,
            },
            ServerKey::TypeScript
            | ServerKey::Pyright
            | ServerKey::Gopls
            | ServerKey::CssLs
            | ServerKey::HtmlLs
            | ServerKey::Eslint => Self {
                enabled: true,
                check_on_save: false,
                format_on_save: false,
            },
        }
    }
}

/// Storage for per-language configs, keyed by the Debug form of
/// `ServerKey` ("RustAnalyzer", "TypeScript", …) so it survives
/// enum-reorderings in the binary.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LanguageConfigs {
    pub configs: HashMap<String, LanguageConfig>,
}

impl LanguageConfigs {
    pub fn get_or_default(&self, key: ServerKey) -> LanguageConfig {
        self.configs
            .get(&format!("{key:?}"))
            .cloned()
            .unwrap_or_else(|| LanguageConfig::defaults_for(key))
    }

    pub fn set(&mut self, key: ServerKey, cfg: LanguageConfig) {
        self.configs.insert(format!("{key:?}"), cfg);
    }
}

pub fn which_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let full = Path::new(dir).join(bin);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}

#[derive(Default)]
pub struct LspManager {
    servers: HashMap<ServerKey, Arc<server::LspServer>>,
    /// Which servers have been notified about a given file. A single file
    /// can be attached to multiple servers (e.g. type-checker + linter).
    files: RwLock<HashMap<PathBuf, Vec<ServerKey>>>,
    pub downloader: Downloader,
    /// Users who declined the install prompt this session — don't nag again
    /// until restart.
    pub declined: HashSet<ServerKey>,
    /// The LSP the app is currently prompting the user to install, if any.
    pub prompt_install: Option<ServerKey>,
    /// Files we've queued up waiting for a server to become available.
    pending_files: RwLock<HashMap<ServerKey, Vec<(PathBuf, String)>>>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn did_open(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        text: &str,
        configs: &LanguageConfigs,
    ) {
        let keys: Vec<ServerKey> = server::keys_for_path(path)
            .into_iter()
            .filter(|k| configs.get_or_default(*k).enabled)
            .collect();
        if keys.is_empty() {
            return;
        }
        self.files.write().insert(path.to_path_buf(), keys.clone());
        for key in keys {
            self.open_on_server(ctx, key, path, text);
        }
    }

    fn open_on_server(
        &mut self,
        ctx: &egui::Context,
        key: ServerKey,
        path: &Path,
        text: &str,
    ) {
        // Evict a dead server so a fresh spawn can replace it (e.g. after
        // the user downloads the binary and wants to retry).
        if let Some(s) = self.servers.get(&key)
            && s.status() == server::Status::Dead
        {
            self.servers.remove(&key);
        }

        if let Some(server) = self.servers.get(&key) {
            server.did_open(path, text);
            return;
        }

        // Prefer a downloaded copy — user explicitly chose it. Fall back
        // to PATH for users with a known-good global install.
        let (cmd, _) = key.command();
        let downloaded = self.downloader.resolved(key);
        let path_bin = which_on_path(cmd);
        let resolved = downloaded.or(path_bin);

        if let Some(bin) = resolved {
            let server = Arc::new(server::LspServer::spawn(ctx.clone(), key, &bin));
            self.servers.insert(key, server.clone());
            server.did_open(path, text);
            return;
        }

        self.pending_files
            .write()
            .entry(key)
            .or_default()
            .push((path.to_path_buf(), text.to_string()));

        if Downloader::is_supported(key)
            && !self.declined.contains(&key)
            && self.prompt_install.is_none()
            && !matches!(
                self.downloader.state(key),
                DownloadState::Downloading { .. } | DownloadState::Ready(_)
            )
        {
            self.prompt_install = Some(key);
        }

        self.try_spawn_pending(ctx, key);
    }

    fn try_spawn_pending(&mut self, ctx: &egui::Context, key: ServerKey) {
        if self.servers.contains_key(&key) {
            return;
        }
        let Some(bin) = self.downloader.resolved(key) else {
            return;
        };
        let server = Arc::new(server::LspServer::spawn(ctx.clone(), key, &bin));
        self.servers.insert(key, server.clone());
        if let Some(queue) = self.pending_files.write().remove(&key) {
            for (path, text) in queue {
                server.did_open(&path, &text);
            }
        }
    }

    /// Called each frame to drain ready downloads into spawned servers,
    /// and to offer the install prompt when a spawn-from-PATH server dies
    /// (rust-analyzer crashes on incompatible workspaces, tsserver refuses
    /// bad installs, etc.).
    pub fn tick(&mut self, ctx: &egui::Context) {
        // Early exit: nothing to do. Avoids scanning servers and hitting
        // locks every single frame when no state transition is possible.
        if self.pending_files.read().is_empty()
            && self.servers.values().all(|s| s.status() != server::Status::Dead)
            && self.prompt_install.is_some()
        {
            // prompt already raised, no pending files, no dead servers.
            return;
        }
        // If a download landed for a key whose server previously died,
        // evict the dead server and re-queue all tracked files so we spawn
        // fresh with the downloaded binary.
        let dead_with_download: Vec<ServerKey> = self
            .servers
            .iter()
            .filter(|(k, s)| {
                s.status() == server::Status::Dead
                    && self.downloader.resolved(**k).is_some()
            })
            .map(|(k, _)| *k)
            .collect();
        for key in dead_with_download {
            self.servers.remove(&key);
            let files: Vec<(PathBuf, String)> = self
                .files
                .read()
                .iter()
                .filter(|(_, ks)| ks.contains(&key))
                .map(|(p, _)| {
                    let text = std::fs::read_to_string(p).unwrap_or_default();
                    (p.clone(), text)
                })
                .collect();
            self.pending_files.write().entry(key).or_default().extend(files);
        }

        let keys: Vec<ServerKey> = self.pending_files.read().keys().copied().collect();
        for key in keys {
            self.try_spawn_pending(ctx, key);
        }
        if self.prompt_install.is_none() {
            for (key, server) in &self.servers {
                if server.status() == server::Status::Dead
                    && Downloader::is_supported(*key)
                    && !self.declined.contains(key)
                    && !matches!(
                        self.downloader.state(*key),
                        DownloadState::Downloading { .. } | DownloadState::Ready(_)
                    )
                {
                    self.prompt_install = Some(*key);
                    break;
                }
            }
        }
    }

    pub fn accept_install(&mut self, ctx: &egui::Context) {
        if let Some(key) = self.prompt_install.take() {
            self.downloader.start_download(key, ctx.clone());
        }
    }

    pub fn decline_install(&mut self) {
        if let Some(key) = self.prompt_install.take() {
            self.declined.insert(key);
        }
    }

    pub fn did_change(&self, path: &Path, text: &str) {
        let keys = match self.files.read().get(path).cloned() {
            Some(ks) => ks,
            None => return,
        };
        for key in keys {
            if let Some(s) = self.servers.get(&key) {
                s.did_change(path, text);
            }
        }
    }

    pub fn did_save(&self, path: &Path, text: &str, configs: &LanguageConfigs) {
        let keys = match self.files.read().get(path).cloned() {
            Some(ks) => ks,
            None => return,
        };
        for key in keys {
            let cfg = configs.get_or_default(key);
            if !cfg.enabled || !cfg.check_on_save {
                continue;
            }
            if let Some(s) = self.servers.get(&key) {
                s.did_save(path, text);
            }
        }
    }

    pub fn is_tracked(&self, path: &Path) -> bool {
        self.files.read().contains_key(path)
    }

    pub fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let keys = match self.files.read().get(path).cloned() {
            Some(ks) => ks,
            None => return Vec::new(),
        };
        let mut out = Vec::new();
        for key in keys {
            if let Some(s) = self.servers.get(&key) {
                out.extend(s.diagnostics_for(path));
            }
        }
        out
    }

    /// Fire-and-forget goto-definition. Dispatches the request against
    /// every server attached to `path`; returns one (ServerKey, id)
    /// token per dispatched server. Caller polls those tokens via
    /// `take_goto_result`. Previously this method blocked the render
    /// thread for up to 1.5s per server — rolled up on slow LSPs.
    pub fn goto_dispatch(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Vec<(ServerKey, i64)> {
        let Some(keys) = self.files.read().get(path).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for key in keys {
            if let Some(s) = self.servers.get(&key)
                && let Some(id) = s.goto_definition_dispatch(path, line, character)
            {
                out.push((key, id));
            }
        }
        out
    }

    /// Outer Some = result available (inner Some = location, inner None
    /// = server returned null). Outer None = still waiting.
    pub fn take_goto_result(
        &self,
        key: ServerKey,
        id: i64,
    ) -> Option<Option<Location>> {
        self.servers.get(&key)?.take_definition_result(id)
    }

    #[allow(dead_code)] // UI wiring deferred; tier-3 LSP feature.
    pub fn hover(&self, path: &Path, line: u32, character: u32) -> Option<String> {
        // Primary server wins for hover — later we could merge
        // markdown blocks from multiple sources.
        let keys = self.files.read().get(path).cloned()?;
        for key in keys {
            if let Some(s) = self.servers.get(&key)
                && let Some(text) = s.hover(path, line, character)
            {
                return Some(text);
            }
        }
        None
    }

    /// Status snapshot of every spawned server — used by Settings → About
    /// to tell the user whether rust-analyzer / tsserver / gopls etc. are
    /// actually running.
    pub fn statuses(&self) -> Vec<(ServerKey, server::Status)> {
        self.servers
            .iter()
            .map(|(k, s)| (*k, s.status()))
            .collect()
    }
}
