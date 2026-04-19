//! One LSP server process with lifecycle, request/response tracking and shared
//! state for diagnostics + hover responses.
//!
//! Threading model:
//! - Main thread writes JSON-RPC messages via `self.stdin` (Mutex).
//! - A background reader thread parses server → client messages and writes
//!   into `Shared` (diagnostics, hover results, init state, notification
//!   signals). It calls `ctx.request_repaint()` so the UI redraws.
//! - Hover is fire-and-wait: the caller records its request id and then
//!   polls `Shared.hover_results` up to a short timeout. Since each call
//!   budgets ~800 ms and hovers are user-triggered, this is OK.

use crate::lsp::protocol;
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ServerKey {
    RustAnalyzer,
    TypeScript,
    Gopls,
    Pyright,
    CssLs,
    HtmlLs,
    /// Secondary analyzer for TS/JS — added to the file's server list only
    /// when an eslint config is detected in the ancestor tree.
    Eslint,
}

/// All server keys that apply to `path`. Multiple entries are returned
/// when a language supports secondary analyzers — e.g. TS gets tsserver
/// for types plus (eventually) eslint-lsp for lint. One enum entry per
/// extension is the MVP; see `keys_for_path_multi` for the layered path.
#[allow(dead_code)] // kept for callers that only need the primary LSP key.
pub fn key_for_path(path: &Path) -> Option<ServerKey> {
    keys_for_path(path).into_iter().next()
}

pub fn keys_for_path(path: &Path) -> Vec<ServerKey> {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return Vec::new();
    };
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => vec![ServerKey::RustAnalyzer],
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => {
            let mut keys = vec![ServerKey::TypeScript];
            if has_eslint_config(path) {
                keys.push(ServerKey::Eslint);
            }
            keys
        }
        "go" => vec![ServerKey::Gopls],
        "py" => vec![ServerKey::Pyright],
        "css" | "scss" | "less" => vec![ServerKey::CssLs],
        "html" | "htm" | "vue" | "svelte" => vec![ServerKey::HtmlLs],
        _ => Vec::new(),
    }
}

fn has_eslint_config(start: &Path) -> bool {
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    const NAMES: &[&str] = &[
        ".eslintrc",
        ".eslintrc.js",
        ".eslintrc.cjs",
        ".eslintrc.mjs",
        ".eslintrc.yaml",
        ".eslintrc.yml",
        ".eslintrc.json",
        "eslint.config.js",
        "eslint.config.cjs",
        "eslint.config.mjs",
        "eslint.config.ts",
    ];
    // `keys_for_path` runs per-frame for every open TS file, so without
    // a cache this would fire ~15 `is_file()` syscalls per ancestor
    // per file per frame. Cache per starting directory for 5 s.
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, (bool, Instant)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));

    let key = start.parent().unwrap_or(start).to_path_buf();
    if let Ok(map) = cache.lock()
        && let Some((hit, t)) = map.get(&key)
        && t.elapsed().as_secs() < 5
    {
        return *hit;
    }

    let found = crate::util::find_ancestor(&key, |dir| {
        NAMES.iter().any(|n| dir.join(n).is_file())
    })
    .is_some();
    if let Ok(mut map) = cache.lock() {
        map.insert(key, (found, Instant::now()));
    }
    found
}

impl ServerKey {
    pub fn command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            ServerKey::RustAnalyzer => ("rust-analyzer", &[]),
            ServerKey::TypeScript => ("typescript-language-server", &["--stdio"]),
            ServerKey::Gopls => ("gopls", &[]),
            ServerKey::Pyright => ("pyright-langserver", &["--stdio"]),
            ServerKey::CssLs => ("vscode-css-language-server", &["--stdio"]),
            ServerKey::HtmlLs => ("vscode-html-language-server", &["--stdio"]),
            ServerKey::Eslint => ("vscode-eslint-language-server", &["--stdio"]),
        }
    }

    pub fn install_hint(self) -> &'static str {
        match self {
            ServerKey::RustAnalyzer => "rustup component add rust-analyzer",
            ServerKey::TypeScript => "npm i -g typescript typescript-language-server",
            ServerKey::Gopls => "go install golang.org/x/tools/gopls@latest",
            ServerKey::Pyright => "npm i -g pyright   (or: pip install pyright)",
            ServerKey::CssLs | ServerKey::HtmlLs | ServerKey::Eslint => {
                "npm i -g vscode-langservers-extracted"
            }
        }
    }

    fn language_id(self, ext: &str) -> &'static str {
        match self {
            ServerKey::RustAnalyzer => "rust",
            ServerKey::TypeScript => match ext {
                "ts" | "mts" | "cts" => "typescript",
                "tsx" => "typescriptreact",
                "jsx" => "javascriptreact",
                _ => "javascript",
            },
            ServerKey::Gopls => "go",
            ServerKey::Pyright => "python",
            ServerKey::CssLs => "css",
            ServerKey::HtmlLs => "html",
            ServerKey::Eslint => match ext {
                "ts" | "mts" | "cts" => "typescript",
                "tsx" => "typescriptreact",
                "jsx" => "javascriptreact",
                _ => "javascript",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Spawned,
    Initializing,
    Ready,
    Dead,
}

/// An LSP `Location` normalized to a local file path + 0-indexed line/col.
#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub line: u32,
    pub col_start: u32,
    pub col_end: u32,
    pub severity: u8, // 1 error, 2 warning, 3 info, 4 hint
    /// Human-readable text from the LSP. Not shown yet (surfaces in a
    /// future hover/inline tooltip).
    #[allow(dead_code)]
    pub message: String,
    /// Which linter produced the message (e.g. "tsserver", "eslint").
    /// Wired for future source-tagged UI.
    #[allow(dead_code)]
    pub source: Option<String>,
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // Hover UI not wired yet; variant reserved for it.
enum RequestKind {
    Hover,
    Definition,
}

struct Shared {
    initialized: bool,
    /// The id of the `initialize` request, so we can distinguish its
    /// response from any other id-bearing message. Previously we set
    /// `initialized = true` on the first id we saw, which raced with
    /// out-of-order hover/definition responses during startup.
    init_request_id: Option<i64>,
    dead: bool,
    pending_opens: Vec<PendingOpen>,
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    hover_results: HashMap<i64, Option<String>>,
    definition_results: HashMap<i64, Option<Location>>,
    /// What kind of response we should parse when this id arrives.
    pending_kinds: HashMap<i64, RequestKind>,
}

struct PendingOpen {
    uri: String,
    language_id: &'static str,
    version: i32,
    text: String,
}

pub struct LspServer {
    key: ServerKey,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    next_id: AtomicI64,
    doc_versions: Mutex<HashMap<String, i32>>,
    _child: Mutex<Option<Child>>,
    _ctx: egui::Context,
}

impl LspServer {
    pub fn spawn(ctx: egui::Context, key: ServerKey, bin: &Path) -> Self {
        let (_cmd_name, args) = key.command();
        // JS/MJS entrypoints (tsserver, pyright) need Node to launch.
        let needs_node = bin
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "js" || e == "mjs");
        let mut cmd = if needs_node {
            let mut c = Command::new("node");
            c.arg(bin).args(args);
            c
        } else {
            let mut c = Command::new(bin);
            c.args(args);
            c
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child_res = cmd.spawn();
        if let Err(ref e) = child_res {
            eprintln!("[lsp] failed to spawn {}: {e}", bin.display());
        }

        let shared = Arc::new((
            Mutex::new(Shared {
                initialized: false,
                init_request_id: None,
                dead: child_res.is_err(),
                pending_opens: Vec::new(),
                diagnostics: HashMap::new(),
                hover_results: HashMap::new(),
                definition_results: HashMap::new(),
                pending_kinds: HashMap::new(),
            }),
            Condvar::new(),
        ));

        let (stdin, stdout, stderr) = match child_res.as_mut() {
            Ok(c) => (c.stdin.take(), c.stdout.take(), c.stderr.take()),
            Err(_) => (None, None, None),
        };
        if let Some(stderr) = stderr {
            let key_label = format!("{key:?}");
            thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                let r = BufReader::new(stderr);
                for line in r.lines().map_while(Result::ok) {
                    eprintln!("[lsp:{key_label}] {line}");
                }
            });
        }

        if let Some(stdout) = stdout {
            let shared2 = shared.clone();
            let ctx2 = ctx.clone();
            thread::spawn(move || {
                let mut r = BufReader::new(stdout);
                loop {
                    match protocol::read(&mut r) {
                        Ok(bytes) => {
                            if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                                handle_message(&shared2, &v);
                                ctx2.request_repaint();
                            }
                        }
                        Err(_) => {
                            let (m, cv) = &*shared2;
                            m.lock().dead = true;
                            cv.notify_all();
                            break;
                        }
                    }
                }
            });
        }

        let server = Self {
            key,
            stdin: Arc::new(Mutex::new(stdin)),
            shared,
            next_id: AtomicI64::new(1),
            doc_versions: Mutex::new(HashMap::new()),
            _child: Mutex::new(child_res.ok()),
            _ctx: ctx,
        };

        server
    }

    /// Walk up from a file path to find the nearest project root. Returns
    /// the directory containing the first found marker, or the file's parent
    /// if nothing is found.
    fn detect_project_root(path: &Path) -> PathBuf {
        let markers = [
            "Cargo.toml",
            "package.json",
            "tsconfig.json",
            "go.mod",
            "pyproject.toml",
            "requirements.txt",
            "setup.py",
            ".git",
        ];
        let mut cur = path.parent().unwrap_or(path).to_path_buf();
        loop {
            for m in &markers {
                if cur.join(m).exists() {
                    return cur;
                }
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
        path.parent().unwrap_or(path).to_path_buf()
    }

    fn is_dead(&self) -> bool {
        self.shared.0.lock().dead
    }

    pub fn status(&self) -> Status {
        let g = self.shared.0.lock();
        if g.dead {
            Status::Dead
        } else if g.initialized {
            Status::Ready
        } else if !g.pending_opens.is_empty() {
            Status::Initializing
        } else {
            Status::Spawned
        }
    }

    fn next_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    fn send(&self, msg: &Value) {
        let mut slot = self.stdin.lock();
        if let Some(stdin) = slot.as_mut()
            && protocol::send(stdin, msg).is_err()
        {
            self.shared.0.lock().dead = true;
            self.shared.1.notify_all();
        }
    }

    /// LSP-spec graceful shutdown: send `shutdown` request, brief wait,
    /// then `exit` notification, then a beat before Drop kills the
    /// child. rust-analyzer otherwise leaves its DB dirty (full
    /// re-index on next open) and typescript-language-server can
    /// leak orphan node processes.
    fn graceful_shutdown(&self) {
        if self.shared.0.lock().dead {
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "shutdown",
            "params": null,
        }));
        std::thread::sleep(Duration::from_millis(200));
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null,
        }));
        std::thread::sleep(Duration::from_millis(50));
    }

    fn send_initialize(&self, root_uri: Option<String>) {
        let id = self.next_id();
        self.shared.0.lock().init_request_id = Some(id);
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri.clone().map(Value::String).unwrap_or(Value::Null),
            "workspaceFolders": root_uri.as_ref().map(|u| {
                json!([{ "uri": u, "name": "root" }])
            }).unwrap_or(Value::Null),
            "capabilities": {
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false, "didSave": true },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "publishDiagnostics": { "relatedInformation": false }
                },
                "workspace": { "workspaceFolders": false }
            },
            "clientInfo": { "name": "crane", "version": env!("CARGO_PKG_VERSION") },
            // rust-analyzer's real error diagnostics ("cannot find X", type
            // mismatches, unused imports) come from cargo check. Without
            // these options rust-analyzer only reports a handful of
            // syntax/name-resolution infos. tsserver and others ignore.
            "initializationOptions": {
                "checkOnSave": true,
                "check": { "command": "check", "extraArgs": [] },
                "cargo": { "allFeatures": false },
                "diagnostics": { "enable": true, "experimental": { "enable": true } }
            },
        });
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": params,
        }));
        let shared = self.shared.clone();
        let stdin = self.stdin.clone();
        thread::spawn(move || {
            let (m, cv) = &*shared;
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut g = m.lock();
            while !g.initialized && !g.dead && Instant::now() < deadline {
                cv.wait_for(&mut g, Duration::from_millis(500));
            }
            if !g.initialized || g.dead {
                return;
            }
            let pending = std::mem::take(&mut g.pending_opens);
            drop(g);
            // Re-check dead before each write: the server may have been
            // dropped while we were waiting on the condvar. Without this
            // check we'd keep writing to a dead process's stdin for up
            // to 8s, producing spurious errors in the log.
            if shared.0.lock().dead {
                return;
            }
            let mut guard = stdin.lock();
            let Some(stdin) = guard.as_mut() else { return };
            let _ = protocol::send(
                stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "initialized",
                    "params": {}
                }),
            );
            for po in pending {
                if shared.0.lock().dead {
                    return;
                }
                let _ = protocol::send(
                    stdin,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didOpen",
                        "params": {
                            "textDocument": {
                                "uri": po.uri,
                                "languageId": po.language_id,
                                "version": po.version,
                                "text": po.text,
                            }
                        }
                    }),
                );
            }
        });
    }

    pub fn did_open(&self, path: &Path, text: &str) {
        if self.is_dead() {
            return;
        }
        // First file open for this server triggers initialize with a
        // discovered project root — rust-analyzer, gopls, tsserver all need
        // this to actually produce diagnostics.
        let needs_init = {
            let g = self.shared.0.lock();
            !g.initialized && g.pending_opens.is_empty()
        };
        if needs_init {
            let root = Self::detect_project_root(path);
            let root_uri = protocol::path_to_uri(&root);
            self.send_initialize(Some(root_uri));
        }

        let uri = protocol::path_to_uri(path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let lang = self.key.language_id(&ext);
        self.doc_versions.lock().insert(uri.clone(), 1);
        let (m, _) = &*self.shared;
        if !m.lock().initialized {
            m.lock().pending_opens.push(PendingOpen {
                uri,
                language_id: lang,
                version: 1,
                text: text.to_string(),
            });
            return;
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": lang,
                    "version": 1,
                    "text": text
                }
            }
        }));
    }

    pub fn did_change(&self, path: &Path, text: &str) {
        if self.is_dead() {
            return;
        }
        let uri = protocol::path_to_uri(path);
        let mut versions = self.doc_versions.lock();
        let version = versions.entry(uri.clone()).or_insert(1);
        *version += 1;
        let v = *version;
        drop(versions);
        // Drop stale diagnostics for this file. Old entries point to text
        // positions that may no longer make sense (e.g. highlighting a
        // comment line when the real error moved). The server will send
        // fresh ones shortly.
        self.shared.0.lock().diagnostics.remove(&uri);
        if !self.shared.0.lock().initialized {
            return;
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": v },
                "contentChanges": [ { "text": text } ]
            }
        }));
    }

    pub fn did_save(&self, path: &Path, text: &str) {
        if self.is_dead() || !self.shared.0.lock().initialized {
            return;
        }
        let uri = protocol::path_to_uri(path);
        // rust-analyzer's `checkOnSave` hook triggers `cargo check` on this
        // notification — that's what produces real compile errors (the
        // kind you see underlined in VSCode). Without it, rust-analyzer
        // only reports a handful of info-level diagnostics.
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didSave",
            "params": {
                "textDocument": { "uri": uri },
                "text": text
            }
        }));
    }

    pub fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        let uri = protocol::path_to_uri(path);
        self.shared
            .0
            .lock()
            .diagnostics
            .get(&uri)
            .cloned()
            .unwrap_or_default()
    }

    #[allow(dead_code)] // queued feature: UI-side dwell detection not yet wired.
    pub fn hover(&self, path: &Path, line: u32, character: u32) -> Option<String> {
        if self.is_dead() || !self.shared.0.lock().initialized {
            return None;
        }
        let id = self.next_id();
        self.shared.0.lock().pending_kinds.insert(id, RequestKind::Hover);
        let uri = protocol::path_to_uri(path);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }
        }));
        let deadline = Instant::now() + Duration::from_millis(800);
        let (m, cv) = &*self.shared;
        let mut g = m.lock();
        while !g.hover_results.contains_key(&id) && Instant::now() < deadline {
            cv.wait_for(&mut g, Duration::from_millis(50));
        }
        g.hover_results.remove(&id).flatten()
    }

    /// Fire-and-forget: sends the definition request and returns the
    /// request id (to be polled via `take_definition_result`). Returns
    /// None if the server isn't usable right now. Non-blocking.
    pub fn goto_definition_dispatch(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Option<i64> {
        if self.is_dead() || !self.shared.0.lock().initialized {
            return None;
        }
        let id = self.next_id();
        self.shared.0.lock().pending_kinds.insert(id, RequestKind::Definition);
        let uri = protocol::path_to_uri(path);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }
        }));
        Some(id)
    }

    /// Outer Option: Some(_) = result arrived and was taken (inner Some =
    /// got a location, inner None = server returned null). Outer None =
    /// still waiting. Non-blocking.
    pub fn take_definition_result(&self, id: i64) -> Option<Option<Location>> {
        let mut g = self.shared.0.lock();
        if g.definition_results.contains_key(&id) {
            Some(g.definition_results.remove(&id).flatten())
        } else {
            None
        }
    }

    /// Legacy synchronous wrapper kept as a short blocking fallback for
    /// code paths that haven't been migrated. New callers should use
    /// the dispatch + poll pair above.
    #[allow(dead_code)]
    pub fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Option<Location> {
        let id = self.goto_definition_dispatch(path, line, character)?;
        let deadline = Instant::now() + Duration::from_millis(1500);
        let (m, cv) = &*self.shared;
        let mut g = m.lock();
        while !g.definition_results.contains_key(&id) && Instant::now() < deadline {
            cv.wait_for(&mut g, Duration::from_millis(50));
        }
        g.definition_results.remove(&id).flatten()
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        self.graceful_shutdown();
    }
}

fn handle_message(shared: &Arc<(Mutex<Shared>, Condvar)>, v: &Value) {
    let (m, cv) = &**shared;
    let method = v.get("method").and_then(|x| x.as_str());
    if let Some("textDocument/publishDiagnostics") = method {
        if let Some(params) = v.get("params") {
            let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            let diags = params
                .get("diagnostics")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(parse_diagnostic)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            m.lock().diagnostics.insert(uri.to_string(), diags);
            cv.notify_all();
        }
        return;
    }
    // Response path: id + result (initialize, hover, definition, etc.)
    if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
        let result = v.get("result");
        let mut g = m.lock();
        // Match the initialize response by its explicit request id —
        // not "first id we see" (which raced with out-of-order
        // hover/definition responses).
        if g.init_request_id == Some(id) {
            g.initialized = true;
        }
        let kind = g.pending_kinds.remove(&id);
        match kind {
            Some(RequestKind::Definition) => {
                let loc = result.and_then(extract_location);
                g.definition_results.insert(id, loc);
            }
            Some(RequestKind::Hover) => {
                let text = result.and_then(extract_hover);
                g.hover_results.insert(id, text);
            }
            None => {
                // Response to an untracked id (initialize, or a stray
                // server-initiated registration/capability reply we
                // don't wait on). Drop — previously this grew
                // hover_results unboundedly.
            }
        }
        cv.notify_all();
    }
}

fn extract_location(result: &Value) -> Option<Location> {
    if result.is_null() {
        return None;
    }
    if let Some(loc) = parse_location(result) {
        return Some(loc);
    }
    if let Some(arr) = result.as_array() {
        for item in arr {
            if let Some(loc) = parse_location(item) {
                return Some(loc);
            }
            if let Some(loc) = parse_location_link(item) {
                return Some(loc);
            }
        }
    }
    None
}

fn parse_location(v: &Value) -> Option<Location> {
    let uri = v.get("uri")?.as_str()?;
    let range = v.get("range")?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let character = start.get("character")?.as_u64()? as u32;
    let path = uri_to_path(uri)?;
    Some(Location { path, line, character })
}

fn parse_location_link(v: &Value) -> Option<Location> {
    let uri = v.get("targetUri")?.as_str()?;
    let range = v
        .get("targetSelectionRange")
        .or_else(|| v.get("targetRange"))?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let character = start.get("character")?.as_u64()? as u32;
    let path = uri_to_path(uri)?;
    Some(Location { path, line, character })
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix("file://")?;
    let mut bytes: Vec<u8> = Vec::with_capacity(stripped.len());
    let raw = stripped.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
            let n = u8::from_str_radix(hex, 16).ok()?;
            bytes.push(n);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

fn parse_diagnostic(v: &Value) -> Option<Diagnostic> {
    let range = v.get("range")?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    let line = start.get("line")?.as_u64()? as u32;
    let col_start = start.get("character")?.as_u64()? as u32;
    let end_line = end.get("line")?.as_u64()? as u32;
    let col_end = if end_line == line {
        end.get("character")?.as_u64()? as u32
    } else {
        u32::MAX
    };
    let severity = v
        .get("severity")
        .and_then(|s| s.as_u64())
        .unwrap_or(1) as u8;
    let message = v
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let source = v
        .get("source")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    Some(Diagnostic {
        line,
        col_start,
        col_end,
        severity,
        message,
        source,
    })
}

fn extract_hover(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = result.get("contents")?;
    match contents {
        Value::String(s) => Some(s.clone()),
        Value::Object(obj) => obj
            .get("value")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(o) => o
                        .get("value")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n\n"))
            }
        }
        _ => None,
    }
}
