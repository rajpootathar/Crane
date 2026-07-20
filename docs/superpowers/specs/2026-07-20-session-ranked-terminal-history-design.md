# Session-ranked terminal history

**Date**: 2026-07-20
**Status**: Approved, pending implementation plan
**Builds on**: `project_terminal_history_scoping` and `project_warp_style_rewrite`
(memory) — this is Phase 0 + a scoped slice of Phase 2 of the Warp-style rewrite.

## Summary

Give every Crane terminal its own command history, ranked the way Warp ranks it:
commands from the current (or restored) session sort to the top, and within each
tier commands typed in the current directory outrank commands from elsewhere.
Nothing is ever hidden — arrow-up still reaches every command; the model reorders,
it does not filter.

This ships as three layers, built bottom-up:

1. **Shell integration** — Crane ships `crane-init.zsh` / `crane-init.bash` that
   emit OSC 633 (VS Code's shell-integration convention) reporting each command,
   its cwd, and its exit code. Loaded via a `ZDOTDIR` shim so the user's own
   `~/.zshrc` is never edited.
2. **`crane_term` OSC 633 parsing** — extend the existing `OscWatcher` to decode
   OSC 633 and surface command/prompt/exit events to the app, reusing the exact
   pipeline OSC 9 / OSC 777 notifications already flow through.
3. **History store + ranked up-arrow** — an append-only store at
   `~/.crane/history/`, and interception of Up/Down in the terminal view to serve
   a ranked, deduped suggestion in place of raw zsh history.

These ship as one unit because layer 3 is unobservable without layers 1–2, and
layers 1–2 have no user-facing value on their own except exit-code metadata.
Splitting them would mean shipping recording with no consumer.

## Why

Reported 2026-07-19 (restated from the parked 2026-05-12 issue): up-arrow in a
Crane terminal shows commands typed in a *different* folder, and a terminal
restored on restart does not show its own prior commands first.

**Root cause.** Crane pipes bytes through `portable-pty` → `$SHELL`. The shell
owns history (`~/.zsh_history`), and every shell instance reads/writes the same
global file; the PTY is byte transport with no concept of "command" or "cwd." A
restored pane spawns a *brand-new* shell process, so even a "restored" terminal is
new to zsh and inherits only the shared global history — never its own past
session. See `controller.rs:63-87` (bare `$SHELL` spawn, four env vars, no shell
integration).

**Why not the quick HISTFILE patch.** The 2026-05-12 note rejected a static
per-project `HISTFILE` swap as throwaway scaffolding, and that still holds for a
*filter*-based fix. This design is different: the durable asset is the **store**
(command + cwd + exit code + session), which is precisely the data layer the
Warp-rewrite Phase 2 editor will read. When Phase 2 lands, the ~40-line shell hook
and the arrow interception are discarded; the store and its schema survive, along
with all accumulated history.

## What Warp actually does (verified from `vendor/warp`)

The design mirrors Warp, and the reference implementation is vendored, so the
model is taken from source rather than guessed:

- `HistoryEntry` (`app/src/terminal/history.rs:254`) records `command`, `pwd`,
  `exit_code`, `session_id`, `start_ts`, `completed_ts` — but ranking **ignores
  `pwd`**.
- `history_order()` (`app/src/input_suggestions.rs:1197`) sorts by a two-value
  enum only:

  ```rust
  pub enum HistoryOrder { DifferentSession, CurrentSession }  // Different < Current
  ```

  Current-session (and restored) commands rank above everything else; ties break
  by timestamp; duplicates are removed keeping the latest occurrence
  (`up_arrow.rs`, `sort_and_dedupe_suggestions`).
- The restart requirement is met by one rule (`input_suggestions.rs:1205`):

  ```rust
  // Restored blocks are always treated as CurrentSession
  if entry.is_for_restored_block { return HistoryOrder::CurrentSession; }
  ```

**Crane's one deliberate divergence.** Warp does not weight by directory at all.
Crane adds `pwd` as a **tie-breaker within each session tier** (see Ranking
below), reconciling the original "per-folder" request with Warp's graceful,
never-hides model. This is the only intentional departure from Warp's algorithm.

## Scope

**In scope**

- `crane-init.zsh` + `crane-init.bash`, loaded via a `ZDOTDIR` shim for zsh and an
  equivalent for bash, emitting OSC 633 A/B/C/D/E/P.
- OSC 633 decoding in `crane_term`, surfaced via a new `Handler` method and a
  `Term` queue, drained by the controller — parallel to the OSC 9/777 path.
- `~/.crane/history/` append-only JSONL store; in-memory load on first use.
- Per-PTY session id, persisted on the pane; restored-session flagging.
- Up/Down interception in `TerminalView::write_keystroke`, gated by alt-screen
  and prompt-active guards, with `^E^U`-then-inject line replacement (emacs mode).
- Graceful fall-through to native zsh history whenever any guard fails.
- Unit tests for OSC 633 parsing, the ranking function, and store round-trip.

**Out of scope (deferred)**

- The full Crane-owned line editor (Warp-rewrite Phase 2, sub-project 3): `^A`/
  `^E`/`^W`/`^R`, completion, multi-line, bracketed paste, Vi-mode editing. This
  design intercepts **only Up/Down** and leaves every other key to zle.
- Vi-mode ranked history. In v1, detecting vi keymap **disables** interception
  (native zsh history serves instead) rather than corrupt the line.
- The block-model UI (collapse/expand, re-run, copy-as-markdown). The OSC 633
  boundaries this design records are its foundation, but no block UI ships here.
- fish and non-zsh/bash shells: they degrade to today's behaviour.
- `Ctrl-G`-style toggle to a global/unranked view. The store keeps everything, so
  this is a later ranking-mode addition, not a data change.

## Architecture

Three layers. Two run at runtime; one is on disk. Each maps to an existing Crane
pattern so no new architectural primitive is introduced.

```
zsh/bash (crane-init.*)                          ~/.crane/history/history.jsonl
      │ emits OSC 633 A/B/C/D/E/P                        ▲ append
      ▼                                                  │
portable-pty ──▶ crane_term OscWatcher ──▶ Handler ──▶ HistoryStore (in-memory + disk)
   (bytes)        (perform.rs, new 633 arm)   │              ▲
                                              ▼              │ rank on demand
                                    Term shell-event queue   │
                                              │              │
                                    controller drains  ─────▶ TerminalView::write_keystroke
                                    (per frame)                (Up/Down interception)
```

### Layer 1 — Shell integration

**Loading without touching the user's rc.** Crane writes its init scripts under
`~/.crane/shell/` at startup (idempotent; overwrite on version change). For zsh it
sets `ZDOTDIR=~/.crane/shell/zsh` on the PTY command (`controller.rs`, beside the
existing `cmd.env(...)` calls). That directory's `.zshrc`:

1. Records the real `ZDOTDIR` (or `$HOME`) as `CRANE_OLD_ZDOTDIR`.
2. Sources the user's real `${CRANE_OLD_ZDOTDIR}/.zshrc` (and `.zshenv`/`.zprofile`
   as appropriate) so the user's environment is fully intact.
3. Sources Crane's hooks last, so they win.

This is VS Code's shell-integration mechanism; it is not Crane-proprietary. For
bash, `--rcfile` (interactive) plays the equivalent role.

**What the hooks emit (OSC 633).** OSC 633 is chosen over bare OSC 133 because 133
does not carry the command *text* — it expects the terminal to scrape it off the
grid. 633;E hands it to us explicitly.

| Sequence | Meaning | Emitted from |
|---|---|---|
| `OSC 633 ; A ST` | prompt start | `precmd` |
| `OSC 633 ; B ST` | command start (prompt end) | `precmd` (end) |
| `OSC 633 ; C ST` | pre-execution | `preexec` |
| `OSC 633 ; D ; <exit> ST` | command finished + exit code | `precmd` (next) |
| `OSC 633 ; E ; <cmdline> ST` | the command line text | `preexec` |
| `OSC 633 ; P ; Cwd=<path> ST` | cwd property | `precmd` / `chpwd` |

The command text in `E` is escaped per VS Code's convention (`;`, newlines, and
`\` encoded) so a command containing `;` cannot corrupt the frame; the parser
reverses it.

### Layer 2 — `crane_term` OSC 633 parsing

Extend `OscWatcher::osc_dispatch` (`crates/crane_term/src/perform.rs:265`) with a
`b"633"` arm that decodes the sub-command byte and dispatches to a new `Handler`
method:

```rust
// handler.rs — default no-op, matching osc_notification's shape
fn shell_integration(&mut self, _event: ShellIntegrationEvent) {}
```

`ShellIntegrationEvent` is a small enum (`PromptStart`, `CommandStart`,
`PreExec`, `CommandFinished { exit: Option<i32> }`, `CommandLine(String)`,
`Cwd(String)`). `Term` buffers these on a `Vec<ShellIntegrationEvent>` queue with a
`take_shell_events()` drain, exactly mirroring `notifications` /
`take_notifications()` (`term.rs:59,164`). The controller drains it on the same
reader tick that already drains notifications (`controller.rs:138-155`) and forwards
into the store. crane_term remains parser-only.

### Layer 3 — History store + session identity

**Store.** `~/.crane/history/history.jsonl`, one JSON object per line:

```json
{"command":"cargo build","pwd":"/Users/x/proj","exit":0,
 "session":"a1b2","start":"2026-07-20T10:15:03Z","end":"2026-07-20T10:15:19Z"}
```

JSONL (not SQLite) keeps to the lean-deps rule — serde/serde_json are already in
the tree (`persist.rs`). Loaded fully into memory on first up-arrow and kept for
the process; appends are written through to disk. At terminal-history scale
(thousands of rows) a linear ranking scan is imperceptible. A single writer per
process serializes appends; concurrent Crane windows are out of scope (last-writer
wins on the append, no interleave corruption because writes are whole lines with
`O_APPEND`).

**Assembling an entry.** The controller correlates events per terminal:
`CommandLine` + `Cwd` captured at `PreExec`, `exit` + `end` captured at the next
`CommandFinished`. Only completed commands are recorded.

**Session identity.** A `session_id` is minted per PTY spawn and stored on the pane
in `warpui-state.json` (panes persist by `PaneId`, `persist.rs:71`). On restart, a
restored pane carries a `restored_session_ids: Vec<SessionId>` — its prior
session(s). During ranking, any entry whose `session` is the live id **or** in the
restored set is treated as `CurrentSession` (Warp's `is_for_restored_block` rule).

### Ranking

Pure function over the in-memory entries, given `(current_session_id,
restored_session_ids, current_pwd)`:

1. **Tier** (primary, descending): `CurrentSession` (live or restored) above
   `DifferentSession`. — Warp parity.
2. **Directory** (secondary, Crane's addition): within a tier, `pwd ==
   current_pwd` above `pwd != current_pwd`.
3. **Recency** (tertiary): later `start` first.
4. **Dedup**: identical command text collapses to its latest occurrence
   (Warp's `sort_and_dedupe_suggestions`).

The function is table-tested independently of any I/O.

### Up-arrow interception

In `TerminalView::write_keystroke` (`view.rs:973`), before encoding Up/Down:

**Guards — all must hold, else fall through to the existing `\x1b[A` / `\x1b[B`:**

1. Not `is_app_cursor()` — no full-screen app (vim/less/htop) owns the screen.
   Method already exists (`view.rs:977`).
2. Shell integration is active for this terminal (we have seen OSC 633 events) and
   the cursor is at an **active prompt** (`CommandStart` seen, no `PreExec` since).
3. Not vi keymap (v1 limitation; see Out of scope).

**Behaviour.** Each terminal keeps a history cursor and the original typed prefix.
On Up, compute the ranked list, advance the cursor, and replace the line by
sending `^E` (`\x05`, end of line) then `^U` (`\x15`, kill to start) then the
chosen command bytes — correct in emacs mode, zsh's default. On Down, walk back
toward the original prefix; past it, restore the prefix and release the cursor.

**Known risk & fallback.** `^E^U` is keymap-sensitive; vi mode rebinds them, which
is why guard 3 disables interception under vi. If the inline `^E^U` approach proves
fragile in practice even under emacs mode, the fallback is a Crane-rendered
suggestion overlay that commits text only on Enter, never driving zle mid-line.
The store, ranking, and OSC layers are unchanged by that fallback — only the
presentation of layer 3 changes.

## Error handling & graceful degradation

Degradation is a **feature**: the change can only ever *add* ranked history, never
break the terminal.

- No hooks sourced (ssh into a remote box, bash without our rcfile, a shell we did
  not instrument) → no OSC 633 → guard 2 fails → today's exact behaviour.
- Vi keymap → guard 3 fails → native zsh history.
- Full-screen app running → guard 1 fails → arrows go to the app verbatim.
- Store unreadable / corrupt line → skip the bad line, log, continue; never block
  input.
- `~/.crane/history/` uncreatable → recording silently disabled; interception
  guard 2 naturally fails; terminal unaffected.

## Testing

- **OSC 633 parsing** — unit tests beside the existing `osc_tests` module
  (`perform.rs:336`): each sub-command, escaped command text with embedded `;` and
  newlines, malformed/short params ignored. Matches the 38-test crane_term style.
- **Ranking** — pure table-driven tests: tier ordering, restored-session
  promotion, pwd tie-break, recency, dedup-keep-latest.
- **Store** — write → reload → rank round-trip; corrupt-line tolerance.
- **Interception guards** — the alt-screen / prompt-active / vi-mode gating, in
  isolation from real PTY I/O where possible.

## Rollout / reversibility

- The shell scripts and the arrow interception are the throwaway parts; the store
  and its schema are the durable Phase-2 asset.
- No migration: absence of `~/.crane/history/` simply means "no history yet."
- A single guard (`shell_integration_active`) disables the entire user-facing
  behaviour if needed, leaving recording intact.

## Open questions (non-blocking)

- **bash coverage depth.** zsh gets first-class `chpwd`-driven cwd reporting; bash
  cwd reporting leans on `PROMPT_COMMAND`. bash is in scope but may land a beat
  behind zsh if its hook proves fiddly.
- **Global-view toggle.** Deferred, but the store already retains everything, so
  adding a `Ctrl-G`-style unranked view later is a ranking-mode change only.
```
