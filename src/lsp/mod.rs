//! LSP client — diagnostics + hover for Rust, TS/JS/TSX/JSX, Go, Python, CSS, HTML.
//!
//! One server process per language, spawned lazily on first file open.
//! Communication is JSON-RPC over stdio (see `protocol.rs`). Server messages
//! are parsed on a background thread and written to a shared state struct;
//! the UI polls that state each repaint.

pub mod protocol;
pub mod server;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use server::{Diagnostic, ServerKey};

#[derive(Default)]
pub struct LspManager {
    servers: HashMap<ServerKey, Arc<server::LspServer>>,
    files: RwLock<HashMap<PathBuf, ServerKey>>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn did_open(&mut self, ctx: &egui::Context, path: &Path, text: &str) {
        let Some(key) = server::key_for_path(path) else {
            return;
        };
        let server = self
            .servers
            .entry(key)
            .or_insert_with(|| Arc::new(server::LspServer::spawn(ctx.clone(), key)))
            .clone();
        server.did_open(path, text);
        self.files.write().insert(path.to_path_buf(), key);
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
