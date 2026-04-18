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
}

pub fn key_for_path(path: &Path) -> Option<ServerKey> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(ServerKey::RustAnalyzer),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => Some(ServerKey::TypeScript),
        "go" => Some(ServerKey::Gopls),
        "py" => Some(ServerKey::Pyright),
        "css" | "scss" | "less" => Some(ServerKey::CssLs),
        "html" | "htm" | "vue" | "svelte" => Some(ServerKey::HtmlLs),
        _ => None,
    }
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
        }
    }

    pub fn install_hint(self) -> &'static str {
        match self {
            ServerKey::RustAnalyzer => "rustup component add rust-analyzer",
            ServerKey::TypeScript => "npm i -g typescript typescript-language-server",
            ServerKey::Gopls => "go install golang.org/x/tools/gopls@latest",
            ServerKey::Pyright => "npm i -g pyright   (or: pip install pyright)",
            ServerKey::CssLs | ServerKey::HtmlLs => {
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

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub line: u32,
    pub col_start: u32,
    pub col_end: u32,
    pub severity: u8, // 1 error, 2 warning, 3 info, 4 hint
    pub message: String,
    pub source: Option<String>,
}

struct Shared {
    initialized: bool,
    dead: bool,
    pending_opens: Vec<PendingOpen>,
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    hover_results: HashMap<i64, Option<String>>,
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
        let mut child_res = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        if let Err(ref e) = child_res {
            eprintln!("[lsp] failed to spawn {}: {e}", bin.display());
        }

        let shared = Arc::new((
            Mutex::new(Shared {
                initialized: false,
                dead: child_res.is_err(),
                pending_opens: Vec::new(),
                diagnostics: HashMap::new(),
                hover_results: HashMap::new(),
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

    fn send_initialize(&self, root_uri: Option<String>) {
        let id = self.next_id();
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
            "initializationOptions": Value::Null,
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

    pub fn hover(&self, path: &Path, line: u32, character: u32) -> Option<String> {
        if self.is_dead() || !self.shared.0.lock().initialized {
            return None;
        }
        let id = self.next_id();
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
    // Response path: id + result (initialize, hover, etc.)
    if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
        let result = v.get("result");
        let mut g = m.lock();
        if !g.initialized {
            // First response carrying an id is our initialize result.
            g.initialized = true;
        }
        if let Some(r) = result
            && let Some(text) = extract_hover(r)
        {
            g.hover_results.insert(id, Some(text));
        } else {
            g.hover_results.insert(id, None);
        }
        cv.notify_all();
    }
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
