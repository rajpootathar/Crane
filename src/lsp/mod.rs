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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use downloader::{DownloadState, Downloader};
pub use server::{Diagnostic, ServerKey};

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
    files: RwLock<HashMap<PathBuf, ServerKey>>,
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

    pub fn did_open(&mut self, ctx: &egui::Context, path: &Path, text: &str) {
        let Some(key) = server::key_for_path(path) else {
            return;
        };
        self.files.write().insert(path.to_path_buf(), key);

        // Evict a dead server so a fresh spawn can replace it (e.g. after
        // the user downloads rust-analyzer via the prompt, we want to re-try
        // spawn with the downloaded binary even though PATH-spawned one died).
        if let Some(s) = self.servers.get(&key)
            && s.status() == server::Status::Dead
        {
            self.servers.remove(&key);
        }

        // Already running? Just forward the open.
        if let Some(server) = self.servers.get(&key) {
            server.did_open(path, text);
            return;
        }

        // Prefer a downloaded copy if present — the user explicitly chose
        // it, often because their global install was broken. Fall back to
        // PATH lookup for users who have a known-good global install.
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

        // Not resolvable. Queue the open for when the server becomes
        // available, and ask the user to opt in to download (if supported
        // and they haven't already declined).
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

        // If a download finished since the last call, spawn now and flush.
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
                .filter(|(_, k)| **k == key)
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
        let key = match self.files.read().get(path).copied() {
            Some(k) => k,
            None => return,
        };
        if let Some(s) = self.servers.get(&key) {
            s.did_change(path, text);
        }
    }

    pub fn is_tracked(&self, path: &Path) -> bool {
        self.files.read().contains_key(path)
    }

    pub fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let key = match self.files.read().get(path).copied() {
            Some(k) => k,
            None => return Vec::new(),
        };
        self.servers
            .get(&key)
            .map(|s| s.diagnostics_for(path))
            .unwrap_or_default()
    }

    pub fn hover(&self, path: &Path, line: u32, character: u32) -> Option<String> {
        let key = self.files.read().get(path).copied()?;
        let server = self.servers.get(&key)?;
        server.hover(path, line, character)
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
