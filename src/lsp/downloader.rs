//! Optional auto-download of LSP server binaries. User opts in via a prompt;
//! binaries land at `~/.crane/lsp/<server>/`.
//!
//! Currently supported: rust-analyzer (single gzipped binary from GitHub
//! releases). gopls, tsserver, pyright need a runtime (Go / Node) and are
//! out of scope for the first cut — they'll be added once we decide
//! whether to shell out to `go install` / `npm i` or bundle a runtime.

use super::server::ServerKey;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

#[derive(Clone, Debug)]
pub enum DownloadState {
    NotStarted,
    Downloading { progress_bytes: u64 },
    Ready(PathBuf),
    Failed(String),
}

#[derive(Default)]
pub struct Downloader {
    states: Arc<Mutex<HashMap<ServerKey, DownloadState>>>,
}

impl Downloader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return Some(path) if the server binary is already downloaded and
    /// ready to spawn. Does not kick off downloads on its own — callers
    /// must explicitly call `start_download`.
    pub fn resolved(&self, key: ServerKey) -> Option<PathBuf> {
        // Fast path — trust the state. If we already marked Ready, don't
        // re-stat the path every frame (this was doing 6 syscalls/frame).
        {
            let g = self.states.lock();
            if let Some(DownloadState::Ready(p)) = g.get(&key) {
                return Some(p.clone());
            }
        }
        // First-lookup path — stat the expected location and promote to
        // Ready if the binary already exists from a prior session.
        let expected = Self::expected_path(key)?;
        if expected.exists() {
            self.states
                .lock()
                .insert(key, DownloadState::Ready(expected.clone()));
            return Some(expected);
        }
        None
    }

    pub fn state(&self, key: ServerKey) -> DownloadState {
        self.states
            .lock()
            .get(&key)
            .cloned()
            .unwrap_or(DownloadState::NotStarted)
    }

    pub fn is_supported(key: ServerKey) -> bool {
        match key {
            ServerKey::RustAnalyzer => true,
            ServerKey::TypeScript
            | ServerKey::Pyright
            | ServerKey::CssLs
            | ServerKey::HtmlLs
            | ServerKey::Eslint => has_npm(),
            ServerKey::Gopls => false,
        }
    }

    pub fn runtime_missing_hint(key: ServerKey) -> Option<&'static str> {
        match key {
            ServerKey::TypeScript
            | ServerKey::Pyright
            | ServerKey::CssLs
            | ServerKey::HtmlLs
            | ServerKey::Eslint
                if !has_npm() =>
            {
                Some("Requires Node.js (npm) — install from https://nodejs.org")
            }
            ServerKey::Gopls => Some("Requires Go — install from https://go.dev/dl/"),
            _ => None,
        }
    }

    pub fn start_download(&self, key: ServerKey, ctx: egui::Context) {
        {
            let mut g = self.states.lock();
            if matches!(
                g.get(&key),
                Some(DownloadState::Downloading { .. } | DownloadState::Ready(_))
            ) {
                return;
            }
            g.insert(key, DownloadState::Downloading { progress_bytes: 0 });
        }
        let states = self.states.clone();
        let ctx2 = ctx.clone();
        thread::spawn(move || {
            let result = match key {
                ServerKey::RustAnalyzer => download_rust_analyzer(&states, key, &ctx2),
                ServerKey::TypeScript => install_npm_server(
                    key,
                    "typescript",
                    &["typescript-language-server", "typescript"],
                ),
                ServerKey::Pyright => install_npm_server(key, "pyright", &["pyright"]),
                ServerKey::CssLs | ServerKey::HtmlLs | ServerKey::Eslint => {
                    install_npm_server(key, "vscode-langservers", &["vscode-langservers-extracted"])
                }
                ServerKey::Gopls => Err(std::io::Error::other(
                    "gopls auto-install not yet supported (needs Go SDK)",
                )),
            };
            let mut g = states.lock();
            match result {
                Ok(p) => {
                    g.insert(key, DownloadState::Ready(p));
                }
                Err(e) => {
                    g.insert(key, DownloadState::Failed(e.to_string()));
                }
            }
            ctx2.request_repaint();
        });
    }

    fn expected_path(key: ServerKey) -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let base = PathBuf::from(home).join(".crane").join("lsp");
        match key {
            ServerKey::RustAnalyzer => {
                let bin = if cfg!(windows) {
                    "rust-analyzer.exe"
                } else {
                    "rust-analyzer"
                };
                Some(base.join("rust-analyzer").join(bin))
            }
            ServerKey::TypeScript => Some(
                base.join("typescript")
                    .join("node_modules/typescript-language-server/lib/cli.mjs"),
            ),
            ServerKey::Pyright => Some(
                base.join("pyright")
                    .join("node_modules/pyright/dist/pyright-langserver.js"),
            ),
            ServerKey::CssLs => Some(
                base.join("vscode-langservers")
                    .join("node_modules/vscode-langservers-extracted/bin/vscode-css-language-server"),
            ),
            ServerKey::HtmlLs => Some(
                base.join("vscode-langservers")
                    .join("node_modules/vscode-langservers-extracted/bin/vscode-html-language-server"),
            ),
            ServerKey::Eslint => Some(
                base.join("vscode-langservers")
                    .join("node_modules/vscode-langservers-extracted/bin/vscode-eslint-language-server"),
            ),
            ServerKey::Gopls => None,
        }
    }
}

pub fn has_npm() -> bool {
    which_on_path("npm").is_some()
}

pub fn has_node() -> bool {
    which_on_path("node").is_some()
}

fn which_on_path(bin: &str) -> Option<PathBuf> {
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

fn download_rust_analyzer(
    states: &Arc<Mutex<HashMap<ServerKey, DownloadState>>>,
    key: ServerKey,
    ctx: &egui::Context,
) -> std::io::Result<PathBuf> {
    let triple = target_triple()
        .ok_or_else(|| std::io::Error::other("unsupported target platform for auto-install"))?;
    let url = format!(
        "https://github.com/rust-lang/rust-analyzer/releases/latest/download/rust-analyzer-{triple}.gz"
    );
    let expected = Downloader::expected_path(key)
        .ok_or_else(|| std::io::Error::other("no HOME dir"))?;
    let dir = expected
        .parent()
        .ok_or_else(|| std::io::Error::other("bad path"))?
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let gz_path = dir.join("rust-analyzer.gz");

    let resp = ureq::get(&url)
        .call()
        .map_err(|e| std::io::Error::other(format!("GET failed: {e}")))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut gz_file = std::fs::File::create(&gz_path)?;
    let mut buf = [0u8; 32 * 1024];
    let mut read_total: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        gz_file.write_all(&buf[..n])?;
        read_total += n as u64;
        states.lock().insert(
            key,
            DownloadState::Downloading {
                progress_bytes: read_total,
            },
        );
        ctx.request_repaint();
        let _ = total; // could be used for percentage
    }
    drop(gz_file);

    let gz = std::fs::File::open(&gz_path)?;
    let mut decoder = flate2::read::GzDecoder::new(gz);
    let mut out = std::fs::File::create(&expected)?;
    std::io::copy(&mut decoder, &mut out)?;
    drop(out);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&expected)?;
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&expected, perms)?;
    }
    let _ = std::fs::remove_file(&gz_path);

    // Macs mark downloaded binaries as quarantined which fails gatekeeper
    // on spawn. Strip it.
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("xattr")
            .args([
                "-d",
                "com.apple.quarantine",
                expected.to_string_lossy().as_ref(),
            ])
            .output();
    }

    Ok(expected)
}

fn install_npm_server(
    key: ServerKey,
    install_subdir: &str,
    packages: &[&str],
) -> std::io::Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| std::io::Error::other("no HOME dir"))?;
    let dir = PathBuf::from(home).join(".crane").join("lsp").join(install_subdir);
    std::fs::create_dir_all(&dir)?;

    // Seed a package.json so `npm install` doesn't complain.
    let pkg_json = dir.join("package.json");
    if !pkg_json.exists() {
        std::fs::write(&pkg_json, br#"{"name":"crane-lsp","private":true}"#)?;
    }

    let mut cmd = std::process::Command::new("npm");
    cmd.arg("install")
        .arg("--prefix")
        .arg(&dir)
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .arg("--no-progress");
    for pkg in packages {
        cmd.arg(*pkg);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "npm install failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    let expected = Downloader::expected_path(key)
        .ok_or_else(|| std::io::Error::other("no expected path"))?;
    if !expected.exists() {
        return Err(std::io::Error::other(format!(
            "installed but entrypoint not found at {}",
            expected.display()
        )));
    }
    Ok(expected)
}

fn target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Some("aarch64-apple-darwin");
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Some("x86_64-apple-darwin");
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return Some("x86_64-unknown-linux-gnu");
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        return Some("aarch64-unknown-linux-gnu");
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return Some("x86_64-pc-windows-msvc");
    }
    #[allow(unreachable_code)]
    None
}

pub fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.0} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

#[allow(dead_code)]
pub fn binary_exists_on_path_or_downloaded(cmd: &str, key: ServerKey, dl: &Downloader) -> Option<PathBuf> {
    if let Some(p) = dl.resolved(key) {
        return Some(p);
    }
    // Check PATH.
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let full = Path::new(dir).join(cmd);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}
