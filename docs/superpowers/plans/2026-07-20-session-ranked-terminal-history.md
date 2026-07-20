# Session-ranked terminal history — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every Crane terminal its own command history — ranked so the current/restored session's commands sort first, current-directory commands next, deduped by recency — recorded via OSC 633 shell integration and served on Up/Down.

**Architecture:** Three layers, built bottom-up. (1) `crane-init.zsh`/`.bash` emit OSC 633 (VS Code convention) reporting command text, cwd, exit code, prompt boundaries. (2) `crane_term`'s `OscWatcher` decodes OSC 633 and surfaces `ShellIntegrationEvent`s through a `Term` queue drained by the PTY reader — the exact pipeline OSC 9/11 already use. (3) A JSONL store at `~/.crane/history/`, per-PTY session identity persisted on the pane, and Up/Down interception in `TerminalView::write_keystroke` that replaces the line with a ranked suggestion.

**Tech Stack:** Rust edition 2024, `crane_term` crate, `portable-pty`, `serde`/`serde_json`, `parking_lot::Mutex`, `std::time::SystemTime`. No new dependencies. No async runtime.

## Global Constraints

- **No new dependencies.** Timestamps use `std::time::SystemTime` → u64 unix-millis (no `chrono`). Serialization uses the already-present `serde` / `serde_json`. — from spec "lean-deps".
- **No async runtime.** Recording happens on the existing PTY reader `std::thread`; ranking is synchronous in the paint/input path. — CLAUDE.md.
- **`crane_term` stays parser-only.** It surfaces events to a `Handler`; it never touches the store, the theme, or Crane types. — CLAUDE.md architecture.
- **Graceful degradation is mandatory.** Any guard failing (no OSC 633 seen, alt-screen app, vi keymap, unreadable store) falls through to today's exact behaviour (`\x1b[A` / `\x1b[B`). The feature only ever *adds* ranked history. — spec "Error handling".
- **Naming glossary** (canonical): Pane, Workspace, Tab, Layout. Do not drift.
- **Commit messages: zero AI references** — no "Claude", no "Co-Authored-By", no bot/assistant mentions. Conventional commits (`feat:`, `test:`, `fix:`). — CLAUDE.md, non-negotiable.
- **Store location:** `~/.crane/history/history.jsonl`. Shell init scripts: `~/.crane/shell/zsh/` and `~/.crane/shell/crane-init.bash`.

---

## File Structure

**Sub-project 1 — Recording**
- `crates/crane_term/src/handler.rs` (modify) — add `ShellIntegrationEvent` enum + `shell_integration(&mut self, ShellIntegrationEvent)` default no-op handler method.
- `crates/crane_term/src/perform.rs` (modify) — add `b"633"` arm to `OscWatcher::osc_dispatch`; decode sub-commands; unit tests.
- `crates/crane_term/src/term.rs` (modify) — `shell_events` queue field, `take_shell_events()`, `shell_integration` impl; unit tests.
- `src/warpui/history_store.rs` (create) — `HistoryEntry`, `HistoryStore` (load/append/rank), process-wide singleton accessor. Owns ranking.
- `assets/shell/crane-init.zsh` (create) — zsh hooks emitting OSC 633.
- `assets/shell/zshrc` (create) — the `ZDOTDIR` shim `.zshrc` that sources the user's real rc then `crane-init.zsh`.
- `assets/shell/crane-init.bash` (create) — bash `PROMPT_COMMAND`/`trap DEBUG` hooks emitting OSC 633.
- `src/warpui/shell_init.rs` (create) — writes the bundled shell scripts to `~/.crane/shell/` at startup (idempotent) and computes the env vars a PTY needs.
- `src/warpui/controller.rs` (modify) — set `ZDOTDIR` (zsh) / rcfile env at spawn; mint a `session_id`; drain `take_shell_events()` in the reader thread; assemble + append `HistoryEntry`s.
- `src/warpui/mod.rs` (modify) — `mod history_store; mod shell_init;`.

**Sub-project 2 — Ranked up-arrow**
- `src/warpui/persist.rs` (modify) — add `session_id` + `restored_session_ids` to `STerminal` (both `#[serde(default)]`).
- `src/warpui/controller.rs` (modify) — expose `session_id()`, `restored_session_ids()`, `cwd()`; accept restored ids on construction.
- `src/warpui/view.rs` (modify) — Up/Down interception + guards + line replacement + per-terminal history-cursor state.
- `src/warpui/history_store.rs` (modify) — `rank()` already lives here from Task 4; add the history-cursor helper if needed.

---

## SUB-PROJECT 1 — RECORDING

### Task 1: OSC 633 events in `crane_term`

**Files:**
- Modify: `crates/crane_term/src/handler.rs`
- Modify: `crates/crane_term/src/perform.rs`
- Modify: `crates/crane_term/src/term.rs`
- Test: inline `#[cfg(test)]` modules in `perform.rs` and `term.rs` (existing `osc_tests` / `tests`).

**Interfaces:**
- Produces:
  - `pub enum crane_term::ShellIntegrationEvent { PromptStart, CommandStart, PreExec, CommandFinished { exit: Option<i32> }, CommandLine(String), Cwd(String) }` (re-exported from `lib.rs`).
  - `Handler::shell_integration(&mut self, event: ShellIntegrationEvent)` — default no-op.
  - `Term::take_shell_events(&mut self) -> Vec<ShellIntegrationEvent>`.

- [ ] **Step 1: Write the failing parse test** (append to `perform.rs`'s `osc_tests` module)

```rust
    // Add to the Collector struct: a `shell_events: Vec<ShellIntegrationEvent>`
    // field, and impl the handler method to collect them.
    fn run_shell_events(bytes: &[u8]) -> Vec<ShellIntegrationEvent> {
        let mut parser = vte::Parser::new();
        let mut sink = Collector::default();
        let mut watcher = OscWatcher { inner: &mut sink };
        parser.advance(&mut watcher, bytes);
        sink.shell_events
    }

    #[test]
    fn osc633_boundaries_and_payloads_decode() {
        use ShellIntegrationEvent::*;
        assert!(matches!(run_shell_events(b"\x1b]633;A\x07").as_slice(), [PromptStart]));
        assert!(matches!(run_shell_events(b"\x1b]633;B\x07").as_slice(), [CommandStart]));
        assert!(matches!(run_shell_events(b"\x1b]633;C\x07").as_slice(), [PreExec]));
        assert!(matches!(
            run_shell_events(b"\x1b]633;D;0\x07").as_slice(),
            [CommandFinished { exit: Some(0) }]
        ));
        assert!(matches!(
            run_shell_events(b"\x1b]633;D;130\x07").as_slice(),
            [CommandFinished { exit: Some(130) }]
        ));
        assert_eq!(
            run_shell_events(b"\x1b]633;E;git commit\x07"),
            vec![CommandLine("git commit".into())]
        );
        assert_eq!(
            run_shell_events(b"\x1b]633;P;Cwd=/Users/x/proj\x07"),
            vec![Cwd("/Users/x/proj".into())]
        );
    }

    #[test]
    fn osc633_command_line_unescapes_semicolons_and_newlines() {
        // VS Code encodes ; as \x3b, newline as \x0a, backslash as \x5c.
        assert_eq!(
            run_shell_events(b"\x1b]633;E;echo a\\x3bb\\x0ac\x07"),
            vec![ShellIntegrationEvent::CommandLine("echo a;b\nc".into())]
        );
    }

    #[test]
    fn osc633_unknown_subcommand_ignored() {
        assert!(run_shell_events(b"\x1b]633;Z;whatever\x07").is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p crane_term osc633 2>&1 | tail -20`
Expected: FAIL — `ShellIntegrationEvent` not found / `shell_integration` not a member.

- [ ] **Step 3: Add the enum + handler method** (in `handler.rs`, near `osc_notification`)

```rust
/// A shell-integration event decoded from an OSC 633 sequence (VS Code's
/// convention). Surfaced to the Handler so Crane can record command history
/// keyed by cwd + exit code + prompt boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellIntegrationEvent {
    /// 633;A — the shell is about to draw a prompt.
    PromptStart,
    /// 633;B — end of prompt / start of editable command region.
    CommandStart,
    /// 633;C — the command is about to execute (leaving the prompt).
    PreExec,
    /// 633;D;<exit> — the command finished with this exit code (None if absent).
    CommandFinished { exit: Option<i32> },
    /// 633;E;<cmdline> — the command line text (already unescaped).
    CommandLine(String),
    /// 633;P;Cwd=<path> — the shell's current working directory.
    Cwd(String),
}
```

Add to the `Handler` trait (default no-op):

```rust
    /// OSC 633 shell-integration event (prompt boundary, command text, cwd,
    /// or exit code). Default no-op; Crane's `Term` buffers these for the
    /// reader thread to record into the history store.
    fn shell_integration(&mut self, _event: ShellIntegrationEvent) {}
```

- [ ] **Step 4: Decode OSC 633 in `OscWatcher::osc_dispatch`** (in `perform.rs`, add an arm before `_ => {}`)

```rust
            // OSC 633 — VS Code shell-integration. Reports prompt boundaries,
            // the command line, cwd, and exit code so Crane can record history.
            b"633" => {
                let Some(sub) = params.get(1).and_then(|p| p.first().copied()) else {
                    return;
                };
                let event = match sub {
                    b'A' => Some(ShellIntegrationEvent::PromptStart),
                    b'B' => Some(ShellIntegrationEvent::CommandStart),
                    b'C' => Some(ShellIntegrationEvent::PreExec),
                    b'D' => {
                        let exit = params
                            .get(2)
                            .and_then(|p| std::str::from_utf8(p).ok())
                            .and_then(|s| s.trim().parse::<i32>().ok());
                        Some(ShellIntegrationEvent::CommandFinished { exit })
                    }
                    b'E' => params
                        .get(2)
                        .map(|p| ShellIntegrationEvent::CommandLine(unescape_osc633(p))),
                    b'P' => params.get(2).and_then(|p| {
                        let s = String::from_utf8_lossy(p);
                        s.strip_prefix("Cwd=")
                            .map(|cwd| ShellIntegrationEvent::Cwd(cwd.to_string()))
                    }),
                    _ => None,
                };
                if let Some(event) = event {
                    self.inner.shell_integration(event);
                }
            }
```

Add the unescape helper at module scope in `perform.rs` (below `join_params_utf8`):

```rust
/// Reverse VS Code's OSC 633 payload escaping: `\xHH` hex escapes back to
/// their byte, so a command line containing `;` (encoded `\x3b`) or a newline
/// (`\x0a`) round-trips intact. Unknown/malformed escapes are passed through
/// literally.
fn unescape_osc633(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if chars.peek() == Some(&'x') {
                chars.next();
                let h1 = chars.next();
                let h2 = chars.next();
                if let (Some(a), Some(b)) = (h1, h2) {
                    if let Ok(byte) = u8::from_str_radix(&format!("{a}{b}"), 16) {
                        out.push(byte as char);
                        continue;
                    }
                }
                // Malformed escape — emit what we consumed literally.
                out.push('\\');
                out.push('x');
                if let Some(a) = h1 { out.push(a); }
                if let Some(b) = h2 { out.push(b); }
                continue;
            }
            if chars.peek() == Some(&'\\') {
                chars.next();
                out.push('\\');
                continue;
            }
        }
        out.push(c);
    }
    out
}
```

- [ ] **Step 5: Add the `Term` queue + impl** (in `term.rs`)

Add field to the `Term` struct (next to `notifications`):

```rust
    /// OSC 633 shell-integration events buffered for the reader thread to drain
    /// into the history store. Same pattern as `notifications`.
    shell_events: Vec<ShellIntegrationEvent>,
```

Initialize in `Term::new` (next to `notifications: Vec::new(),`):

```rust
            shell_events: Vec::new(),
```

Add the drain method (next to `take_notifications`):

```rust
    /// Drain buffered OSC 633 shell-integration events. Called by the PTY
    /// reader each pass; empty when the shell has no integration sourced.
    pub fn take_shell_events(&mut self) -> Vec<ShellIntegrationEvent> {
        std::mem::take(&mut self.shell_events)
    }
```

Impl the handler method (in `impl Handler for Term`, next to `osc_notification`):

```rust
    fn shell_integration(&mut self, event: ShellIntegrationEvent) {
        self.shell_events.push(event);
    }
```

Add the import at the top of `term.rs` (extend the existing `use crate::handler::...`):

```rust
use crate::handler::ShellIntegrationEvent;
```

Re-export from `lib.rs` (extend the existing `pub use handler::...` or add):

```rust
pub use handler::ShellIntegrationEvent;
```

- [ ] **Step 6: Write the `Term` queue test** (append to `term.rs` `tests`)

```rust
    #[test]
    fn shell_events_buffer_and_drain() {
        use crate::handler::ShellIntegrationEvent::*;
        let mut t = Term::new(5, 10);
        t.shell_integration(PromptStart);
        t.shell_integration(CommandLine("ls -la".into()));
        let drained = t.take_shell_events();
        assert_eq!(drained, vec![PromptStart, CommandLine("ls -la".into())]);
        assert!(t.take_shell_events().is_empty(), "drain must empty the queue");
    }
```

- [ ] **Step 7: Run all crane_term tests**

Run: `cargo test -p crane_term 2>&1 | tail -8`
Expected: PASS — previous 79 + the new OSC 633 tests, 0 failed.

- [ ] **Step 8: Commit**

```bash
git add crates/crane_term/src/handler.rs crates/crane_term/src/perform.rs crates/crane_term/src/term.rs crates/crane_term/src/lib.rs
git commit -m "feat(crane_term): decode OSC 633 shell-integration events"
```

---

### Task 2: History store module

**Files:**
- Create: `src/warpui/history_store.rs`
- Modify: `src/warpui/mod.rs` (add `mod history_store;`)
- Test: inline `#[cfg(test)]` in `history_store.rs`.

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces:
  - `pub struct HistoryEntry { pub command: String, pub pwd: String, pub exit_code: Option<i32>, pub session_id: u64, pub start_ms: u64, pub end_ms: u64 }` (derives `Serialize, Deserialize, Clone, Debug, PartialEq`).
  - `pub struct HistoryStore { entries: Vec<HistoryEntry>, path: PathBuf }` with:
    - `pub fn load(path: PathBuf) -> HistoryStore`
    - `pub fn append(&mut self, entry: HistoryEntry)`
    - `pub fn rank(&self, current: u64, restored: &HashSet<u64>, pwd: &str) -> Vec<&HistoryEntry>`
  - `pub fn store() -> &'static Mutex<HistoryStore>` — process-wide singleton at `~/.crane/history/history.jsonl`.
  - `pub fn now_ms() -> u64` — `SystemTime` unix-millis helper.

- [ ] **Step 1: Write the failing ranking + roundtrip tests** (create `src/warpui/history_store.rs` with just this test module first)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cmd: &str, pwd: &str, sess: u64, ts: u64) -> HistoryEntry {
        HistoryEntry {
            command: cmd.into(),
            pwd: pwd.into(),
            exit_code: Some(0),
            session_id: sess,
            start_ms: ts,
            end_ms: ts + 1,
        }
    }

    #[test]
    fn rank_puts_current_session_above_others_then_by_recency() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("old_other", "/a", 2, 100),
            entry("cur_early", "/a", 1, 200),
            entry("cur_late", "/a", 1, 300),
        ];
        let restored: HashSet<u64> = HashSet::new();
        let ranked: Vec<&str> = s.rank(1, &restored, "/a").iter().map(|e| e.command.as_str()).collect();
        // Current session (1) first, most-recent-first within the tier; other last.
        assert_eq!(ranked, vec!["cur_late", "cur_early", "old_other"]);
    }

    #[test]
    fn rank_promotes_restored_session_into_the_current_tier() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("live", "/a", 1, 100),
            entry("restored", "/a", 9, 200),
            entry("stranger", "/a", 5, 300),
        ];
        let mut restored = HashSet::new();
        restored.insert(9);
        let ranked: Vec<&str> = s.rank(1, &restored, "/a").iter().map(|e| e.command.as_str()).collect();
        // restored(9) ranks in the current tier above stranger(5), by recency.
        assert_eq!(ranked, vec!["restored", "live", "stranger"]);
    }

    #[test]
    fn rank_breaks_ties_by_matching_pwd_within_a_tier() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("here", "/proj", 1, 100),
            entry("elsewhere", "/other", 1, 200), // newer but wrong dir
        ];
        let restored = HashSet::new();
        let ranked: Vec<&str> = s.rank(1, &restored, "/proj").iter().map(|e| e.command.as_str()).collect();
        // Same session tier: current-dir wins over a newer different-dir command.
        assert_eq!(ranked, vec!["here", "elsewhere"]);
    }

    #[test]
    fn rank_dedupes_keeping_the_latest_occurrence() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("ls", "/a", 1, 100),
            entry("ls", "/a", 1, 300),
            entry("pwd", "/a", 1, 200),
        ];
        let restored = HashSet::new();
        let ranked: Vec<&str> = s.rank(1, &restored, "/a").iter().map(|e| e.command.as_str()).collect();
        assert_eq!(ranked, vec!["ls", "pwd"], "duplicate ls collapses to its latest");
    }

    #[test]
    fn append_then_reload_roundtrips_and_skips_corrupt_lines() {
        let dir = std::env::temp_dir().join(format!("crane-hist-test-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.jsonl");
        {
            let mut s = HistoryStore::load(path.clone());
            s.append(entry("cargo build", "/proj", 1, 100));
            s.append(entry("cargo test", "/proj", 1, 200));
        }
        // Inject a corrupt line between good ones.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "{{ not valid json").unwrap();
        }
        let reloaded = HistoryStore::load(path.clone());
        assert_eq!(reloaded.entries.len(), 2, "corrupt line skipped, good lines kept");
        assert_eq!(reloaded.entries[1].command, "cargo test");
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin crane history_store 2>&1 | tail -20`
Expected: FAIL — `HistoryStore` / `HistoryEntry` not found.

- [ ] **Step 3: Implement the module** (prepend above the test module in `history_store.rs`)

```rust
//! Command-history store for terminal panes. One append-only JSONL log at
//! `~/.crane/history/history.jsonl`; each entry records the command, cwd,
//! exit code, the owning session, and timestamps. Ranking (not filtering)
//! decides up-arrow order — current/restored session first, current dir next,
//! then recency, deduped keeping the latest occurrence. Mirrors Warp's model
//! (see vendor/warp `HistoryOrder`), plus a pwd tie-break Warp omits.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub command: String,
    pub pwd: String,
    pub exit_code: Option<i32>,
    pub session_id: u64,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub struct HistoryStore {
    entries: Vec<HistoryEntry>,
    path: PathBuf,
}

/// Milliseconds since the Unix epoch. `0` if the clock is before the epoch
/// (never in practice) — a monotonic-enough ordering key without `chrono`.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl HistoryStore {
    /// Load every well-formed JSONL entry from `path`; a missing file is an
    /// empty store, and a corrupt line is skipped (never aborts the load).
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self { entries, path }
    }

    /// Append `entry` to memory and to disk (one JSON line, `O_APPEND` so
    /// concurrent terminals never interleave a partial line). Disk failure is
    /// swallowed — history is best-effort, never blocks the terminal.
    pub fn append(&mut self, entry: HistoryEntry) {
        if let Ok(line) = serde_json::to_string(&entry) {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
        self.entries.push(entry);
    }

    /// Ranked, deduped view for up-arrow. Order:
    /// 1. session tier — `current` or in `restored` outrank everyone else;
    /// 2. directory — within a tier, `pwd`-match outranks non-match;
    /// 3. recency — later `start_ms` first;
    /// then duplicates by command text collapse to their latest occurrence.
    pub fn rank(&self, current: u64, restored: &HashSet<u64>, pwd: &str) -> Vec<&HistoryEntry> {
        let tier = |e: &HistoryEntry| -> u8 {
            if e.session_id == current || restored.contains(&e.session_id) { 1 } else { 0 }
        };
        let mut idx: Vec<usize> = (0..self.entries.len()).collect();
        idx.sort_by(|&a, &b| {
            let (ea, eb) = (&self.entries[a], &self.entries[b]);
            // All keys descending (higher = ranks first).
            tier(eb)
                .cmp(&tier(ea))
                .then_with(|| {
                    let (pa, pb) = ((ea.pwd == pwd) as u8, (eb.pwd == pwd) as u8);
                    pb.cmp(&pa)
                })
                .then_with(|| eb.start_ms.cmp(&ea.start_ms))
        });
        let mut seen: HashSet<&str> = HashSet::new();
        idx.into_iter()
            .map(|i| &self.entries[i])
            .filter(|e| seen.insert(e.command.as_str()))
            .collect()
    }
}

/// Process-wide store, lazily loaded from `~/.crane/history/history.jsonl`.
pub fn store() -> &'static Mutex<HistoryStore> {
    static STORE: OnceLock<Mutex<HistoryStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".crane")
            .join("history")
            .join("history.jsonl");
        Mutex::new(HistoryStore::load(path))
    })
}
```

- [ ] **Step 4: Register the module** (in `src/warpui/mod.rs`, alongside the other `mod` lines)

```rust
mod history_store;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --bin crane history_store 2>&1 | tail -12`
Expected: PASS — all 5 store tests.

- [ ] **Step 6: Commit**

```bash
git add src/warpui/history_store.rs src/warpui/mod.rs
git commit -m "feat(warpui): add ranked command-history store"
```

---

### Task 3: Shell integration scripts + `shell_init` writer

**Files:**
- Create: `assets/shell/crane-init.zsh`
- Create: `assets/shell/zshrc`
- Create: `assets/shell/crane-init.bash`
- Create: `src/warpui/shell_init.rs`
- Modify: `src/warpui/mod.rs` (add `pub mod shell_init;`)
- Test: inline `#[cfg(test)]` in `shell_init.rs`.

**Interfaces:**
- Produces:
  - `pub fn install_shell_scripts()` — writes the three bundled scripts to `~/.crane/shell/` (idempotent).
  - `pub fn zsh_zdotdir() -> PathBuf` — `~/.crane/shell/zsh`.
  - `pub fn bash_rcfile() -> PathBuf` — `~/.crane/shell/crane-init.bash`.

- [ ] **Step 1: Write `assets/shell/crane-init.zsh`** (the OSC 633 emitter)

```zsh
# Crane shell integration (zsh). Emits OSC 633 (VS Code convention) so Crane
# can record per-directory, session-scoped command history. Safe to source in
# any zsh; a non-Crane terminal simply ignores the escape sequences.

# Guard against double-sourcing.
if [[ -n "$CRANE_SHELL_INTEGRATION" ]]; then
  return 0
fi
CRANE_SHELL_INTEGRATION=1

__crane_osc() { printf '\e]633;%s\a' "$1"; }

# Escape a command line for OSC 633;E: backslash, semicolon, newline.
__crane_escape() {
  local s=${1//\\/\\x5c}
  s=${s//;/\\x3b}
  s=${s//$'\n'/\\x0a}
  printf '%s' "$s"
}

__crane_precmd() {
  local exit=$?
  # Report the just-finished command's exit code (skip on the very first prompt).
  if [[ -n "$__crane_executing" ]]; then
    __crane_osc "D;$exit"
    __crane_executing=""
  fi
  __crane_osc "P;Cwd=$PWD"
  __crane_osc "A"   # prompt start
  __crane_osc "B"   # command start
}

__crane_preexec() {
  __crane_osc "E;$(__crane_escape "$1")"
  __crane_osc "C"   # pre-execution
  __crane_executing=1
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd __crane_precmd
add-zsh-hook preexec __crane_preexec
```

- [ ] **Step 2: Write `assets/shell/zshrc`** (the ZDOTDIR shim)

```zsh
# Crane ZDOTDIR shim. Crane points ZDOTDIR here so it can load its shell
# integration WITHOUT editing the user's own ~/.zshrc. We restore the real
# ZDOTDIR, source the user's normal startup files, then load Crane's hooks.

CRANE_ZDOTDIR="$ZDOTDIR"
if [[ -n "$CRANE_OLD_ZDOTDIR" ]]; then
  export ZDOTDIR="$CRANE_OLD_ZDOTDIR"
else
  unset ZDOTDIR
fi

# Source the user's real interactive rc (best effort).
[[ -f "${ZDOTDIR:-$HOME}/.zshrc" ]] && source "${ZDOTDIR:-$HOME}/.zshrc"

# Load Crane's integration last so its hooks win.
[[ -f "$CRANE_ZDOTDIR/crane-init.zsh" ]] && source "$CRANE_ZDOTDIR/crane-init.zsh"
```

- [ ] **Step 3: Write `assets/shell/crane-init.bash`**

```bash
# Crane shell integration (bash). Emits OSC 633 via PROMPT_COMMAND + a DEBUG
# trap. Sourced via --rcfile after the user's ~/.bashrc.
if [[ -n "$CRANE_SHELL_INTEGRATION" ]]; then
  return 0
fi
CRANE_SHELL_INTEGRATION=1

[[ -f "$HOME/.bashrc" ]] && source "$HOME/.bashrc"

__crane_osc() { printf '\e]633;%s\a' "$1"; }
__crane_escape() {
  local s=${1//\\/\\x5c}; s=${s//;/\\x3b}; s=${s//$'\n'/\\x0a}; printf '%s' "$s"
}

__crane_prompt() {
  local exit=$?
  if [[ -n "$__crane_executing" ]]; then __crane_osc "D;$exit"; __crane_executing=""; fi
  __crane_osc "P;Cwd=$PWD"; __crane_osc "A"; __crane_osc "B"
}
__crane_debug() {
  # Fires before each command; $BASH_COMMAND is the command about to run.
  [[ "$BASH_COMMAND" == "__crane_prompt" ]] && return
  if [[ -z "$__crane_executing" ]]; then
    __crane_osc "E;$(__crane_escape "$BASH_COMMAND")"; __crane_osc "C"; __crane_executing=1
  fi
}
PROMPT_COMMAND="__crane_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
trap '__crane_debug' DEBUG
```

- [ ] **Step 4: Write the failing `shell_init` test** (create `src/warpui/shell_init.rs` with the test first)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_writes_all_three_scripts() {
        // Redirect HOME to a temp dir so we don't touch the real ~/.crane.
        let tmp = std::env::temp_dir().join(format!(
            "crane-shellinit-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("HOME");
        // SAFETY: single-threaded test; restored below.
        unsafe { std::env::set_var("HOME", &tmp); }

        install_shell_scripts();

        assert!(tmp.join(".crane/shell/zsh/crane-init.zsh").exists());
        assert!(tmp.join(".crane/shell/zsh/.zshrc").exists());
        assert!(tmp.join(".crane/shell/crane-init.bash").exists());
        assert_eq!(zsh_zdotdir(), tmp.join(".crane/shell/zsh"));

        match prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
```

- [ ] **Step 5: Run to verify it fails**

Run: `cargo test --bin crane shell_init 2>&1 | tail -12`
Expected: FAIL — `install_shell_scripts` not found.

- [ ] **Step 6: Implement `shell_init.rs`** (prepend above the test module)

```rust
//! Installs Crane's bundled shell-integration scripts under `~/.crane/shell/`
//! at startup and reports the env a PTY needs to load them. The scripts are
//! embedded at compile time (`include_str!`) and rewritten on every launch so
//! a Crane upgrade always ships the current hooks. Editing the user's own
//! rc files is deliberately avoided — zsh loads ours via a ZDOTDIR shim.

use std::path::PathBuf;

const ZSH_INIT: &str = include_str!("../../assets/shell/crane-init.zsh");
const ZSH_RC: &str = include_str!("../../assets/shell/zshrc");
const BASH_INIT: &str = include_str!("../../assets/shell/crane-init.bash");

fn shell_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".crane")
        .join("shell")
}

/// The `ZDOTDIR` Crane points zsh at (contains our `.zshrc` shim).
pub fn zsh_zdotdir() -> PathBuf {
    shell_root().join("zsh")
}

/// The `--rcfile` Crane starts bash with.
pub fn bash_rcfile() -> PathBuf {
    shell_root().join("crane-init.bash")
}

/// Write (or overwrite) the bundled scripts. Idempotent, best-effort — a write
/// failure just means shell integration is unavailable this run.
pub fn install_shell_scripts() {
    let zdot = zsh_zdotdir();
    let _ = std::fs::create_dir_all(&zdot);
    let _ = std::fs::write(zdot.join("crane-init.zsh"), ZSH_INIT);
    let _ = std::fs::write(zdot.join(".zshrc"), ZSH_RC);
    let _ = std::fs::write(bash_rcfile(), BASH_INIT);
}
```

- [ ] **Step 7: Register the module + call install at startup**

In `src/warpui/mod.rs`:

```rust
pub mod shell_init;
```

Find where the app initializes (the `CraneShellView::new` / `new_with_state` entry, `shell.rs`) and add near the top of construction:

```rust
        crate::warpui::shell_init::install_shell_scripts();
```

- [ ] **Step 8: Run the test + build**

Run: `cargo test --bin crane shell_init 2>&1 | tail -8 && cargo build --bin crane 2>&1 | tail -3`
Expected: test PASS; build Finished.

- [ ] **Step 9: Commit**

```bash
git add assets/shell src/warpui/shell_init.rs src/warpui/mod.rs src/warpui/shell.rs
git commit -m "feat(warpui): install OSC 633 shell-integration scripts at startup"
```

---

### Task 4: Spawn with integration + record entries

**Files:**
- Modify: `src/warpui/controller.rs`
- Test: manual + a small unit test for entry assembly (extract the correlation logic into a testable helper).

**Interfaces:**
- Consumes: `crane_term::ShellIntegrationEvent`, `Term::take_shell_events`, `history_store::{store, HistoryEntry, now_ms}`, `shell_init::{zsh_zdotdir, bash_rcfile}`.
- Produces:
  - `TerminalController::session_id(&self) -> u64`
  - A private `ShellRecorder` that folds events → `HistoryEntry`s.

- [ ] **Step 1: Write the failing recorder test** (add a `#[cfg(test)]` in `controller.rs`)

```rust
#[cfg(test)]
mod recorder_tests {
    use super::*;
    use crane_term::ShellIntegrationEvent::*;

    #[test]
    fn recorder_emits_one_entry_per_completed_command() {
        let mut rec = ShellRecorder::new(42);
        // Prompt, cwd, command typed, executes, finishes.
        rec.feed(Cwd("/proj".into()));
        rec.feed(CommandLine("cargo build".into()));
        rec.feed(PreExec);
        let out = rec.feed(CommandFinished { exit: Some(0) });
        let e = out.expect("a completed command yields an entry");
        assert_eq!(e.command, "cargo build");
        assert_eq!(e.pwd, "/proj");
        assert_eq!(e.exit_code, Some(0));
        assert_eq!(e.session_id, 42);
    }

    #[test]
    fn recorder_ignores_a_finish_with_no_command() {
        let mut rec = ShellRecorder::new(1);
        // A bare prompt with no command typed (user hit Enter on empty line).
        assert!(rec.feed(CommandFinished { exit: Some(0) }).is_none());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin crane recorder 2>&1 | tail -12`
Expected: FAIL — `ShellRecorder` not found.

- [ ] **Step 3: Implement `ShellRecorder`** (add to `controller.rs`)

```rust
use crane_term::ShellIntegrationEvent;
use crate::warpui::history_store::{now_ms, HistoryEntry};

/// Folds a terminal's OSC 633 event stream into completed `HistoryEntry`s.
/// Tracks the in-flight command's text, cwd, and start time between `PreExec`
/// and the next `CommandFinished`; a finish with no pending command (empty
/// Enter) yields nothing.
struct ShellRecorder {
    session_id: u64,
    cwd: String,
    pending_command: Option<String>,
    start_ms: u64,
}

impl ShellRecorder {
    fn new(session_id: u64) -> Self {
        Self { session_id, cwd: String::new(), pending_command: None, start_ms: 0 }
    }

    /// Feed one event; returns a completed entry on `CommandFinished` (if a
    /// command was in flight), else `None`.
    fn feed(&mut self, event: ShellIntegrationEvent) -> Option<HistoryEntry> {
        match event {
            ShellIntegrationEvent::Cwd(p) => { self.cwd = p; None }
            ShellIntegrationEvent::CommandLine(c) => {
                self.pending_command = Some(c);
                self.start_ms = now_ms();
                None
            }
            ShellIntegrationEvent::CommandFinished { exit } => {
                let command = self.pending_command.take()?;
                let trimmed = command.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(HistoryEntry {
                    command: trimmed.to_string(),
                    pwd: self.cwd.clone(),
                    exit_code: exit,
                    session_id: self.session_id,
                    start_ms: self.start_ms,
                    end_ms: now_ms(),
                })
            }
            // Prompt boundaries carry no data we persist directly.
            ShellIntegrationEvent::PromptStart
            | ShellIntegrationEvent::CommandStart
            | ShellIntegrationEvent::PreExec => None,
        }
    }
}
```

- [ ] **Step 4: Run the recorder test**

Run: `cargo test --bin crane recorder 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 5: Mint a session id + set spawn env** (in `TerminalController::new`, near the `cmd.env(...)` block)

```rust
        // A unique id for THIS shell session, stamped onto every command it
        // records so ranking can float the current session's history to the
        // top of up-arrow. Monotonic per process; uniqueness across restarts
        // comes from the wall-clock component.
        let session_id = {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            crate::warpui::history_store::now_ms()
                .wrapping_shl(16)
                | (SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff)
        };
```

Add shell-integration env (mirror the `crane-init` loader) after the existing `cmd.env` calls:

```rust
        // Load Crane's shell integration without touching the user's rc.
        // zsh: point ZDOTDIR at our shim dir (its .zshrc sources the real rc
        // then our hooks). bash: --rcfile. Other shells degrade to no history.
        if shell.ends_with("zsh") {
            if let Some(old) = std::env::var_os("ZDOTDIR") {
                cmd.env("CRANE_OLD_ZDOTDIR", old);
            }
            cmd.env("ZDOTDIR", crate::warpui::shell_init::zsh_zdotdir());
        } else if shell.ends_with("bash") {
            cmd.arg("--rcfile");
            cmd.arg(crate::warpui::shell_init::bash_rcfile());
        }
        cmd.env("CRANE_SESSION_ID", session_id.to_string());
```

Store `session_id` on the returned `Self` (add the field to the struct + the constructor tail), and add the accessor:

```rust
    pub fn session_id(&self) -> u64 {
        self.session_id
    }
```

Note: `shell` is currently a `let shell = ...` inside the `#[cfg(unix)]` block; hoist it so it is in scope where the env is set (bind it once above the `CommandBuilder` match).

- [ ] **Step 6: Drain events → record, in the reader thread** (in the reader loop, next to `notes = t.take_notifications();`)

Add a clone of the recorder into the thread and, in the drain block:

```rust
                                shell_events = t.take_shell_events();
```

After the `notes` handling, feed the recorder and append completed entries:

```rust
                            for ev in shell_events {
                                if let Some(entry) = recorder.feed(ev) {
                                    crate::warpui::history_store::store().lock().append(entry);
                                }
                            }
```

Declare `let mut recorder = ShellRecorder::new(session_id);` before the reader loop, and add `shell_events` to the `let (...)` binding tuple + a default in the else arms.

- [ ] **Step 7: Build + run store/recorder tests**

Run: `cargo build --bin crane 2>&1 | tail -3 && cargo test --bin crane recorder 2>&1 | tail -5`
Expected: build Finished; recorder tests PASS.

- [ ] **Step 8: Manual smoke test**

Run the app (`cargo run --bin crane`), open a terminal, run `echo hello`, `pwd`, `ls`. Then:
Run (in another shell): `tail -3 ~/.crane/history/history.jsonl`
Expected: three JSON lines with the commands, `pwd`, `exit_code`, a `session_id`.

- [ ] **Step 9: Commit**

```bash
git add src/warpui/controller.rs
git commit -m "feat(warpui): record terminal command history via OSC 633"
```

---

## SUB-PROJECT 2 — RANKED UP-ARROW

### Task 5: Session identity persistence

**Files:**
- Modify: `src/warpui/persist.rs` (extend `STerminal`)
- Modify: `src/warpui/controller.rs` (accept + expose restored ids)
- Modify: `src/warpui/shell.rs` (thread `session_id` into the saved `STerminal`; feed restored id back on restore)
- Test: inline serde default test in `persist.rs`.

**Interfaces:**
- Consumes: `TerminalController::session_id`.
- Produces:
  - `STerminal { …, session_id: u64, restored_session_ids: Vec<u64> }` (both `#[serde(default)]`).
  - `TerminalController::restored_session_ids(&self) -> &[u64]`.

- [ ] **Step 1: Write the failing backward-compat test** (in `persist.rs` tests)

```rust
    #[test]
    fn sterminal_without_session_fields_still_deserializes() {
        // A state file written before this feature has no session fields.
        let json = r#"{"cwd":"/proj","history":"abc"}"#;
        let t: STerminal = serde_json::from_str(json).unwrap();
        assert_eq!(t.session_id, 0);
        assert!(t.restored_session_ids.is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin crane sterminal_without_session 2>&1 | tail -10`
Expected: FAIL — no field `session_id`.

- [ ] **Step 3: Extend `STerminal`** (in `persist.rs`)

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct STerminal {
    pub cwd: PathBuf,
    #[serde(default)]
    pub history: String,
    /// The session id this terminal ran under when saved. On restore it moves
    /// into the new session's `restored_session_ids` so the terminal's own
    /// prior commands rank as current-session (Warp's `is_for_restored_block`).
    #[serde(default)]
    pub session_id: u64,
    /// Session ids inherited across earlier restarts, accumulated so a chain of
    /// restarts keeps every prior life of this terminal in the current tier.
    #[serde(default)]
    pub restored_session_ids: Vec<u64>,
}
```

- [ ] **Step 4: Run the compat test**

Run: `cargo test --bin crane sterminal_without_session 2>&1 | tail -6`
Expected: PASS.

- [ ] **Step 5: Populate on save** — find where `STerminal` is built for persistence (`shell.rs`, the `term_cache.insert(*id, STerminal { cwd, history })` site) and set the session fields from the live controller:

```rust
                cache.insert(*id, crate::warpui::persist::STerminal {
                    cwd,
                    history,
                    session_id: /* controller.session_id() for this pane */ sid,
                    restored_session_ids: /* controller.restored_session_ids().to_vec() */ prior,
                });
```

- [ ] **Step 6: Feed restored ids on restore** — where a persisted `STerminal` is replayed into a new terminal (`shell.rs`, the restore path that calls the controller with `cwd, history`), pass `session_id` + `restored_session_ids` (its own id folded in) so the new controller carries them. Add the constructor param + field + accessor in `controller.rs`:

```rust
    // TerminalController::new gains: restored: Vec<u64>
    // store it, and:
    pub fn restored_session_ids(&self) -> &[u64] {
        &self.restored_session_ids
    }
```

The restore call assembles `restored = { st.restored_session_ids ∪ {st.session_id} }` (skip 0).

- [ ] **Step 7: Build + tests**

Run: `cargo build --bin crane 2>&1 | tail -3 && cargo test --bin crane persist 2>&1 | tail -6`
Expected: build Finished; persist tests PASS.

- [ ] **Step 8: Commit**

```bash
git add src/warpui/persist.rs src/warpui/controller.rs src/warpui/shell.rs
git commit -m "feat(warpui): persist terminal session identity for restored history"
```

---

### Task 6: Up/Down interception + line replacement

**Files:**
- Modify: `src/warpui/view.rs` (`TerminalView::write_keystroke` + new per-terminal history-cursor state)
- Test: inline unit test for the guard predicate + the history-cursor walk (extract both into pure helpers).

**Interfaces:**
- Consumes: `history_store::store`, `TerminalController::{session_id, restored_session_ids, cwd}`, `is_app_cursor()`.
- Produces: interception behaviour; a pure `HistoryNav` helper.

- [ ] **Step 1: Write the failing history-cursor test** (in `view.rs` tests, or a small `history_nav` submodule)

```rust
#[cfg(test)]
mod history_nav_tests {
    use super::*;

    #[test]
    fn up_walks_back_through_ranked_list_then_stops_at_oldest() {
        let ranked = vec!["c".to_string(), "b".to_string(), "a".to_string()]; // newest-first
        let mut nav = HistoryNav::new();
        assert_eq!(nav.up(&ranked), Some("c"));
        assert_eq!(nav.up(&ranked), Some("b"));
        assert_eq!(nav.up(&ranked), Some("a"));
        assert_eq!(nav.up(&ranked), Some("a"), "past the oldest, stay on oldest");
    }

    #[test]
    fn down_returns_toward_the_original_line_then_clears() {
        let ranked = vec!["c".to_string(), "b".to_string()];
        let mut nav = HistoryNav::new();
        nav.up(&ranked); nav.up(&ranked); // at "b"
        assert_eq!(nav.down(&ranked), Some("c"));
        assert_eq!(nav.down(&ranked), Some(""), "below newest → the (empty) original line");
        assert_eq!(nav.down(&ranked), None, "already at the original line");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin crane history_nav 2>&1 | tail -12`
Expected: FAIL — `HistoryNav` not found.

- [ ] **Step 3: Implement `HistoryNav`** (in `view.rs`)

```rust
/// Per-terminal up/down cursor over a ranked history list. `-1` means "on the
/// user's original (unsubmitted) line"; 0..n indexes the ranked list
/// (newest-first). `up` moves toward older, clamping at the oldest; `down`
/// moves toward the original line and returns `""` when it lands back on it,
/// then `None` once already there.
#[derive(Default)]
struct HistoryNav {
    idx: i32,
}

impl HistoryNav {
    fn new() -> Self {
        Self { idx: -1 }
    }

    fn reset(&mut self) {
        self.idx = -1;
    }

    fn up<'a>(&mut self, ranked: &'a [String]) -> Option<&'a str> {
        if ranked.is_empty() {
            return None;
        }
        self.idx = (self.idx + 1).min(ranked.len() as i32 - 1);
        ranked.get(self.idx as usize).map(|s| s.as_str())
    }

    fn down<'a>(&mut self, ranked: &'a [String]) -> Option<&'a str> {
        if self.idx < 0 {
            return None;
        }
        self.idx -= 1;
        if self.idx < 0 {
            Some("")
        } else {
            ranked.get(self.idx as usize).map(|s| s.as_str())
        }
    }
}
```

- [ ] **Step 4: Run the nav test**

Run: `cargo test --bin crane history_nav 2>&1 | tail -6`
Expected: PASS.

- [ ] **Step 5: Wire interception into `write_keystroke`** (in `view.rs`)

Add a `RefCell<HistoryNav>` field to `TerminalView` (interior-mutable, like `dimmed`). Then at the top of `write_keystroke`, before the generic `keystroke_to_pty_bytes`:

```rust
        // Ranked-history interception. Only for a bare Up/Down at an active
        // shell prompt — never in a full-screen app, and never in vi keymap
        // (its ^E/^U mean something else). Any guard failing falls through to
        // the normal cursor-key escape below.
        let is_up = ks.key == "up" && !ks.ctrl && !ks.alt && !ks.shift;
        let is_down = ks.key == "down" && !ks.ctrl && !ks.alt && !ks.shift;
        if (is_up || is_down) && ctrl.shell_integration_active() && !ctrl.term.lock().is_app_cursor()
        {
            let pwd = ctrl.cwd().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let ranked: Vec<String> = {
                let s = crate::warpui::history_store::store().lock();
                let restored: std::collections::HashSet<u64> =
                    ctrl.restored_session_ids().iter().copied().collect();
                s.rank(ctrl.session_id(), &restored, &pwd)
                    .iter()
                    .map(|e| e.command.clone())
                    .collect()
            };
            let mut nav = self.history_nav.borrow_mut();
            let chosen = if is_up { nav.up(&ranked) } else { nav.down(&ranked) };
            if let Some(text) = chosen {
                // Clear the current line then type the chosen command. ^E (end),
                // ^U (kill to start) works in emacs keymap (zsh default). Guard
                // 3 (vi) already excluded above.
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"\x05\x15"); // ^E ^U
                bytes.extend_from_slice(text.as_bytes());
                ctrl.write_input(&bytes);
            }
            return;
        }
```

Reset the nav whenever a non-arrow key is typed (so editing a recalled command and hitting Up again restarts from the top). At the end of `write_keystroke`, for the normal path:

```rust
        self.history_nav.borrow_mut().reset();
```

- [ ] **Step 6: Add `shell_integration_active` + `cwd` to the controller** (in `controller.rs`)

Set a flag the first time any `ShellIntegrationEvent` is seen in the reader thread (an `Arc<AtomicBool>` like `bell`), and expose:

```rust
    pub fn shell_integration_active(&self) -> bool {
        self.shell_integration_active.load(std::sync::atomic::Ordering::Relaxed)
    }
```

`cwd()` already exists (used by `view.rs:1102`); reuse it. Track vi-mode by watching for the `\x1b]633` never carrying a keymap hint — v1 relies on the emacs default; DECRQM vi detection is deferred per the spec, so no extra work here beyond the ctrl/alt/shift guard.

- [ ] **Step 7: Build + all history tests**

Run: `cargo build --bin crane 2>&1 | tail -3 && cargo test --bin crane 'history' 2>&1 | tail -8`
Expected: build Finished; history_nav + history_store tests PASS.

- [ ] **Step 8: Manual end-to-end smoke test**

Run `cargo run --bin crane`. In a terminal: `echo one`, `echo two`, `cd` elsewhere, `echo three`. Press Up repeatedly:
Expected: current-session commands appear newest-first; commands typed in the current directory rank above ones from the other directory; Down walks back to the empty prompt. Open `vim`, press Up — normal vim behaviour (interception skipped). Quit and relaunch Crane; in the restored terminal press Up — its own prior commands appear at the top.

- [ ] **Step 9: Commit**

```bash
git add src/warpui/view.rs src/warpui/controller.rs
git commit -m "feat(warpui): rank terminal history on up/down arrow"
```

---

## Self-Review

**Spec coverage:**
- Layer 1 (shell OSC 633, ZDOTDIR shim, zsh+bash) → Task 3. ✓
- Layer 2 (crane_term OSC 633 parse, Handler method, Term queue, reader drain) → Task 1 + Task 4 Step 6. ✓
- Layer 3 store (JSONL, load/append) → Task 2. ✓
- Session identity + restored-session rule → Task 4 (mint) + Task 5 (persist/restore). ✓
- Ranking (session tier → pwd tie-break → recency → dedup) → Task 2 `rank` + tests. ✓
- Up/Down interception, guards (alt-screen, prompt-active, vi via modifier guard), `^E^U` line replacement → Task 6. ✓
- Graceful degradation → guards in Task 6 Step 5 + best-effort store in Task 2. ✓
- Testing (OSC parse, ranking, store round-trip, guards) → Tasks 1, 2, 6. ✓

**Deferred per spec (not planned, intentionally):** full editor (`^R`, completion, multi-line), vi-mode ranked history, block UI, fish, global-view toggle.

**Known follow-ups surfaced during planning (call out at execution, do not silently skip):**
- Live theme switch does not re-notify a running shell of new colours — same limitation already noted for OSC 11; out of scope here.
- `cwd()` currently derives from the terminal's own tracking; once OSC 633 `P;Cwd` lands, prefer it as the authoritative cwd. Task 6 uses `ctrl.cwd()`; if that proves stale at the prompt, switch the recorder's `cwd` field to feed a controller-visible current-cwd cell.
- The exact save/restore call sites in `shell.rs` (Task 5 Steps 5–6) must be located in-code; the plan names the anchor (`term_cache.insert … STerminal { cwd, history }`) but the surrounding restore wiring should be read before editing.

**Type consistency:** `HistoryEntry` fields (`command`, `pwd`, `exit_code`, `session_id`, `start_ms`, `end_ms`) are identical across Tasks 2, 4, 5. `rank(current, restored, pwd)` signature identical in Task 2 and its Task 6 caller. `ShellIntegrationEvent` variants identical across Tasks 1 and 4. `HistoryNav::{new, up, down, reset}` consistent in Task 6.
