# Crane — Old-egui → warpui Parity Master Punchlist

## Executive summary

**~205 actionable parity gaps** survive after deduping 8 cross-domain duplicates and dropping ~5 intentional warpui divergences (listed at the end).

| Severity | Count | Meaning |
|---|---|---|
| **High** | 25 | Data-loss, P0 agent-native UX, or a whole feature missing |
| **Medium** | ~83 | Real capability/correctness loss users will hit |
| **Low** | ~97 | Cosmetic / label / metric drift, minor affordances |

**Two structural facts drive the plan:**
1. The new UI collapsed ~11 old modules into one **`src/warpui/shell.rs`** monolith. Most fixes touch it → they largely **must be serialized**.
2. The per-pane/element renderers (`diff_view`, `editor_view`, `markdown_view`, `controller`, `view`, `git`, gutter/scrollbar/find-bar elements) are **separate files → safe to fix in parallel**, then batch their thin shell.rs action-arm wiring in one serial pass.

### TOP 10 most-worth-fixing (highest user impact)
1. **[H] Confirm-delete does permanent `remove_dir_all`/`remove_file`** instead of `trash::delete` — unrecoverable data loss. (Files FS tree / Modals)
2. **[H] Remove-worktree runs `git worktree remove --force` with no confirm** and no dirty/unpushed warning — silently discards a branch + uncommitted work. (Modals / Left Panel)
3. **[H] Close-tab tears down layout + PTYs with no confirmation.** (Modals / Left Panel)
4. **[H] OSC 9 / OSC 777 notifications entirely absent** — no in-app toast, no OS banner. This is the P0 agent-CLI (Claude Code Stop/Notification hook) path. (Modals / Persistence)
5. **[H] No filesystem watcher** — agent/external edits don't refresh Changes/diff/file-tree until a manual reload. (Persistence)
6. **[H] Clickable local file paths gone from terminal** (only URLs scan now) — no hover-underline, no click-to-open, no `:LINE:COL`. Core agent workflow. (Terminal)
7. **[M→correctness] Changes parse dropped `-z`** — non-ASCII/space paths get octal-escaped, so `git add -- <path>` fails ("pathspec did not match"). (Changes/Git)
8. **[H] Format-on-save gone** (prettier/rustfmt/ruff/gofmt) — files never auto-format. (Editor)
9. **[H] Git-Log reduced to plain `git log --oneline --graph` text** — lost lane graph, ref pills, refs column, details+diff-on-select, context menu, filter, auto-reload. (Doc panes)
10. **[H] In-app staged auto-updater gone** — only a passive "Update available" string; CLAUDE.md flags the updater as load-bearing. (Persistence / Modals)

Runners-up worth flagging: non-Latin fallback fonts (CJK/RTL tofu), per-hunk stage/unstage, embedded browser pane, PDF viewer, git-op status not scoped per-repo (stale failure bleeds across projects).

---

## 1. Left Panel / Projects tree
**High**
- **[H] Sidebar drag-drop reorder — absent.** Old: full TreeDrag/DropZone payloads, drop-line indicator, scope validation, reorder handlers for root projects/groups/workspaces/tabs. New: click-only, no reorder at all. `src/ui/projects.rs:22-60,1291-1400` → absent.
- **[H] Attention/notification pulse glow (AttentionViz) — absent.** Old: per-Tab `attention_since` pulse aggregated onto collapsed workspace/project/group rows. New: no attention concept. `src/ui/projects.rs:104-115,289-300` → absent.
- **[H] Folder-group header context menu — absent.** Old: right-click → Highlight color palette / Default color / Remove folder group (atomic member removal). New: header has no right-click handler. `src/ui/projects.rs:323-366` → `shell.rs:4201-4211`.

**Medium**
- **[M] Per-group tint (`group_tints`) — absent.** Header icon+label rendered in plain `theme::text()`; no group_tints map. `projects.rs:281-317` → `shell.rs:4196-4211`.
- **[M] Atomic-group rule not enforced.** Old hid individual "Remove Project" for a multi-member group member (`allow_individual_remove=false`). New always shows it → can half-empty a group. `projects.rs:419-432` → `shell.rs:1402-1406`.

**Low**
- **[L] Middle-click a Tab row to close — absent.** `projects.rs:1002-1005` → `shell.rs:4021-4100`.
- **[L] F2 / Cmd+R rename active Tab chord — absent.** `projects.rs:1040-1049` → absent.
- **[L] Reveal drifted:** old canonicalizes symlinks + `open <dir>`; new `open -R <path>` highlights the worktree inside `~/.crane-worktrees` (the "not meaningful" parent old avoided). `projects.rs:79-102` → `shell.rs:7446-7454`.
- **[L] Worktree menu order/labels drifted:** Rename moved out of first slot, "Highlight color" label dropped, swatch glyph CUBE vs GIT_BRANCH. `projects.rs:806-856` → `shell.rs:1666-1715`.
- **[L] Project menu drifted:** Initialize Git now above the swatch row, "Highlight color" label dropped, Remove uses TRASH vs X. `projects.rs:556-614` → `shell.rs:1318-1406`.
- **[L] Tab menu drifted:** label dropped, swatch glyph CUBE vs TERMINAL_WINDOW. `projects.rs:1054-1104` → `shell.rs:1719-1755`.
- **[L] Affordance model drifted:** old hover-revealed trailing PLUS/X buttons; new uses always-visible "New workspace"/"New tab" rows, removal is menu-only. `projects.rs:472-503` → `shell.rs:4437-4464`.
- **[L] Group glyph swaps FOLDER↔FOLDER_OPEN** (old kept FOLDER + caret only). `projects.rs:301-317` → `shell.rs:4196-4211`.
- **[L] Project row lit as active** (old explicitly did NOT tint project header; new comment "mirrors old" is wrong). `projects.rs:445-461` → `shell.rs:4227-4252`.
- **[L] Add-Project footer restyled** (pill "Add Project" vs full-width primary "Add Project…/Choose a folder"). `projects.rs:136-156` → `shell.rs:4466-4508`.
- **[L] PROJECTS header size/color/spacing drift.** `projects.rs:161-171` → `shell.rs:4149-4157`.

*(Remove-worktree confirm ↔ §8; Close-tab confirm ↔ §8.)*

---

## 2. Right Panel — Changes + Git
**Medium**
- **[M/correctness] `-z` NUL parsing dropped** → escaped non-ASCII/space paths break `git add -- <path>`. Old used `-z` specifically to fix this. `src/git.rs:98-104,117-145` → `warpui/git.rs:304-351`.
- **[M] `--untracked-files=all` dropped** → a new dir collapses to one `dir/` row; Changes tree and Files tree disagree on its contents. `git.rs:85-108` → `warpui/git.rs:292-300`.
- **[M] Tri-state file checkbox → single `staged` bool + PLUS/MINUS glyph.** An `MM` file now shows MINUS and click UNSTAGES (old showed indeterminate and click STAGED the remainder) — opposite action. `ui/explorer.rs:898-931` → `shell.rs:5017-5045`.
- **[M] git-op status not scoped to repo.** `OpStatus` has no repo field + shared `Arc<Mutex>` → a Push failure/pill in project A persists into project B's Changes tab and disables its buttons. `explorer.rs:238-247,487-515` → `warpui/git.rs:616-637`.
- **[M] Multi-line git-op error pill gone.** Old: first stderr line + chevron to expand full stderr (per-repo persisted). New: one truncated line, no expand. `explorer.rs:650-723` → `shell.rs:5185-5188`.
- **[M] Multi-line commit message lost.** Old: `TextEdit::multiline`, submit on Cmd+Enter. New: fake single-line, bare Enter submits, no newline possible. `explorer.rs:403-423` → `shell.rs:6231-6241`.
- **[M] Push regressions.** Old: `stdin(null)` (fail fast on cred prompt) + returned real ref-update summary. New: generic `run()` doesn't null stdin (can block on askpass) + hard-codes "Pushed". `git.rs:563-605` → `warpui/git.rs:19-32,689`.

**Low**
- **[L] Folder-group checkbox has no indeterminate state** (`any_staged` computed → bound to `_any_staged`, unused). `explorer.rs:826-862` → `shell.rs:4938-4959`.
- **[L] Rename `old -> new` display dropped** in Changes tree. `git.rs:60-71,137-145` → `warpui/git.rs:337-344`.
- **[L] Row context menu offers only one of Stage/Unstage** (single bool) — an `MM` file can't stage its worktree change from the menu. `explorer.rs:935-947` → `shell.rs:1465-1479`.
- **[L] Active tab-chip 2px accent underline gone** (color-only). `explorer.rs:193-202` → `shell.rs:4520-4537`.
- **[L] Push/Pull/Fetch hover tooltips gone.** `explorer.rs:283-324` → `shell.rs:4837-4875`.
- **[L] Toolbar buttons stay clickable + no accent-border on running button** (old truly disabled + 1px accent stroke). `explorer.rs:41-74` → `shell.rs:4837-4875`.
- **[L] Loose-project Changes chip has no "No git in this project" tooltip.** `explorer.rs:130-140` → `shell.rs:4734-4747`.
- **[L] Empty states collapsed:** old distinguished "No active worktree" / "(not a git repo)" / "working tree clean"; new always "working tree clean". `explorer.rs:216-230` → `shell.rs:4676-4682`.
- **[L] Status glyph priority changed** — `MD` shows `D` (new fixed priority) vs `M` (old staged-side-wins). `git.rs:167-185` → `warpui/git.rs:319-336`.
- **[L] Branch label not bold** in Changes toolbar. `explorer.rs:251-256` → `shell.rs:4763-4775`.

*(Per-hunk stage/unstage ↔ §6.)*

---

## 3. Right Panel — Files FS tree
**High**
- **[H/data-loss] Permanent delete instead of Trash.** New `ConfirmDelete` → `fs::remove_dir_all`/`remove_file`; `trash` crate unused. No recovery. `modals/confirm_delete_file.rs:61` → `shell.rs:7471-7482`.
- **[H] Internal FS drag-drop move — absent.** Old: every row drag source + drop target, `fs::rename` with undo push. New `file_row` wires only click/menu. `explorer.rs:1360-1445` → `shell.rs:4551-4639`.

**Medium**
- **[M] Alt-drag = copy + `<name> (n)` de-dupe naming — absent.** `explorer.rs:1555-1593` → absent.
- **[M] Drop-target row highlight — absent** (no drag). `explorer.rs:1394-1427` → absent.
- **[M] External Finder→tree drop (copy OS files into root) — absent** (`dropped_files` never read). `explorer.rs:1134-1165` → absent.
- **[M] "Open Diff" context item for changed files — absent.** `explorer.rs:1474-1483` → `shell.rs:1505-1547`.
- **[M] Right-click empty space → New File/Folder at root — absent** (no hit sink). `explorer.rs:1056-1080` → `shell.rs:4697-4711`.
- **[M] Single-click preview vs double-click permanent gone** — always opens permanently, no preview tab, no `single_click_open` config. `explorer.rs:1345-1359` → `shell.rs:7642-7645`.
- **[M] Deleting a file doesn't close its editor tabs** (only clears `selected_file`) → stale open tab. `confirm_delete_file.rs` → `shell.rs:7471-7482`.

**Low**
- **[L] Floating drag chip near cursor — absent.** `explorer.rs:1179-1245` → absent.
- **[L] Delete dialog wording** now "cannot be undone" (correct given the regression, but reflects trash→permanent). `confirm_delete_file.rs:18-46` → `shell.rs:1759-1816`.
- **[L] Untracked glyph `U` → `?`** in Files tree. `explorer.rs:27-35` → `shell.rs:4576-4579`.
- **[L] "Reveal in File Manager" → "Reveal in Finder"** wording. `explorer.rs:1497` → `shell.rs:1516`.
- **[L] Files-row menu order reshuffled.** `explorer.rs:1459-1515` → `shell.rs:1505-1547`.

*(Cmd+Z file-op undo ↔ §9.)*

---

## 4. Terminal
**High**
- **[H] Clickable local file paths — absent** (URLs only). No `scan_paths`/PathHit, no hover underline, no click-to-open, no in-workspace routing, no `:LINE:COL`. `terminal/view.rs:98-242,1000-1092` → `warpui/view.rs:9-68`.

**Medium**
- **[M] Per-line trailing-whitespace trim on copy gone** → padded TUI copies drag trailing spaces. `view.rs:1403-1424` → `warpui/view.rs:264-266`.
- **[M] Image paste (Cmd+V image → temp PNG → bracketed paste) gone** — text-only paste. Agent-workflow-relevant. `view.rs:1364-1376` → `warpui/view.rs:751-766` *(↔ §9)*.
- **[M] `has_foreground_process` proxied by `is_alt_screen()`** — a non-alt foreground process (REPL/`tail -f`/build) now looks idle, so Cmd+K uses wrong regime. `foreground_process_name`/`_is_cli_agent` also gone. `terminal/term.rs:371-441` → `warpui/controller.rs:244-278`.
- **[M] Sub-row pixel-smooth wheel scroll gone** — rows paint at integer origin only (16px snapping). `view.rs:816-868` → `warpui/grid_element.rs:281-313`.
- **[M] Per-pane terminal tab strip — absent** (one terminal/pane; no spawn-in-pane/rename/duplicate/close-others/middle-click). `view.rs:339-729` → absent *(↔ §11 persistence)*.
- **[M] Scrollbar drifted:** old overlay 4/8px hover-widen, alpha/accent states, hide-when-empty, 20px min. New fixed 12px Flex sibling, always-drawn thumb, flat color, 28px min. `view.rs:1288-1345` → `warpui/scrollbar_element.rs:157-239`.
- **[M] `xterm-crane` terminfo advertising DEC 2026 Sync gone** — hardcodes `xterm-256color`, so Ink TUIs won't emit `?2026`; regresses the duplicate-prompt scenario the migration targeted. `term.rs:21-59,188-200` → `warpui/controller.rs:63`.
- **[M] Terminal font is system Menlo (`.expect()`), not bundled JetBrains Mono** — different metrics/coverage. `view.rs:738` → `warpui/view.rs:191-193` *(↔ §11 fonts)*.

**Low**
- **[L] I-beam/Text cursor over terminal body gone.** `view.rs:793-796` → `warpui/grid_element.rs:490-522`.
- **[L] `TERM_PROGRAM_VERSION` env dropped.** `term.rs:202-203` → `warpui/controller.rs:63-66`.
- **[L] `CRANE_VT_TRACE` byte-trace diagnostic gone.** `term.rs:264-306` → `warpui/controller.rs:96-144`.
- **[L] macOS `malloc_zone_pressure_relief` on drop gone.** `term.rs:106-127` → `warpui/controller.rs:281-294`.

---

## 5. Editor / Files pane
**High**
- **[H] Format-on-save (prettier/rustfmt/ruff/gofmt) — absent.** New save only optionally trims + `fs::write`. `file_save.rs:20-52`, `format/mod.rs:92-133` → `editor_view.rs:702-724`.

**Medium**
- **[M] Save-failure banner gone** (bool ignored, no `save_error`). `file_view.rs:447-467` → `editor_view.rs:713-723`.
- **[M] Silent auto-reload of clean buffers lost** — banner now shows on any mtime diff even when buffer is clean. `file_save.rs:55-96` → `editor_view.rs:762-772,2274-2278`.
- **[M] Diagnostic severity pills + click-to-jump gone** from status strip. `file_status.rs:56-108,158-186` → `shell.rs:5383-5411`.
- **[M] Path breadcrumb + Save/Unlock/Preview header row gone.** `file_view.rs:493-550` → `editor_view.rs:2167-2317`.
- **[M] Editor right-click menu (Save/Reveal/Copy Path/Unlock) — absent.** `file_view.rs:1564-1597` → absent.
- **[M] Gutter git-diff hover tooltip gone** (`dispatch_event` no-op). `file_view.rs:1715-1860` → `gutter_element.rs:214-221`.
- **[M] Scrollbar diagnostic + git-change minimap markers gone.** `file_view.rs:1862-1900` → `scrollbar_element.rs`.
- **[M] Syntax fallback map for flavored ext gone** (h→C, zsh→bash, tsx→TS, vue→HTML …) → plain text. `file_view.rs:106-122` → `editor_view.rs:70-98`.
- **[M] Incremental per-line highlight → whole-file re-highlight per keystroke** on paint thread (perf on large files). `file_view.rs:739-794` → `editor_view.rs:2399-2439`.
- **[M] Read-only affordances gone** — no lock icon/red tint/Unlock button/"Read Only"/menu-unlock; flag gates edits invisibly. `file_view.rs:285-292,505-520` → `shell.rs:5635-5695`.
- **[M] Image files open as garbled text** (no image branch). `file_view.rs:830-860` → `shell.rs:5839-5851`.
- **[M] Dirty-tab close confirmation gone** — X drops unsaved edits silently. `file_view.rs:174-219` → `shell.rs:5681-5693,7320-7343`.

**Low**
- **[L] External-change "Overwrite" action + save-refusal gone** (only Reload/Keep). `file_view.rs:418-441` → `editor_view.rs:2327-2353`.
- **[L] Language + indent labels gone** from status. `file_status.rs:235-329` → `shell.rs:5383-5411`.
- **[L] Current-line background band gone** (only gutter number brightens). `file_view.rs:1684-1710` → `gutter_element.rs:187-192`.
- **[L] Light-theme syntect selection gone** (dark-only fallback). `file_view.rs:688-707` → `editor_view.rs:54-66`.
- **[L] Preview-tab dim styling not read by shell tab strip.** `file_view.rs:283,1904-1907` → `shell.rs:5635-5695`.
- **[L] Diff/read-only tab icons gone** (only dirty dot). `file_view.rs:285-293` → `shell.rs:5645-5660`.
- **[L] Markdown in-editor Preview/Edit toggle gone** — `.md` opens read-only Markdown pane only. `file_view.rs:534-547` → `shell.rs:5805-5837`.
- **[L] Find bar not a real editable field** (append/pop only; no caret/mid-edit/selection/paste). `file_find.rs:38-56` → `editor_view.rs:1719-1763`.
- **[L] Diagnostic info-severity color + solid underline drifted** (dashed; sev3→muted not accent). `diagnostics_overlay.rs:44-71` → `editor_view.rs:104-113,1539-1542`.
- **[L] Cmd+hover goto-def underline hint gone** (Cmd+Click still works). `file_view.rs:1362-1421` → `editor_view.rs:308-316`.
- **[L] Goto-line over-range no longer clamps to last real line.** `file_view.rs:656-665` → `editor_view.rs:1444-1460`.
- **[L] Goto-line bar uses wrong glyph** (ARROW_CLOCKWISE). `file_view.rs:637-641` → `find_bar_element.rs:265-267`.

*Verify:* LSP `did_save` on Cmd+S may be missing (`notify_saved` unported). `file_save.rs:51` → `editor_view.rs:702-724`.

---

## 6. Doc panes (diff / markdown / browser / pdf / git-log)
**High**
- **[H] Diff per-line syntax highlighting gone** — every line one flat color. `diff_view.rs:618-744` → `warpui/diff_view.rs:206-208`.
- **[H] Diff per-hunk stage/unstage gutter gone** — read-only, no git hunk plumbing, no connector lines. `diff_view.rs:556-715` → absent *(↔ §2)*.
- **[H] Browser: embedded WKWebView gone** — 3-line placeholder. `browser_view.rs:236-330` → `shell.rs:6117-6128`.
- **[H] Browser: per-pane tab strip gone.** `browser_view.rs:31-143` → absent.
- **[H] Browser: URL toolbar (back/fwd/reload/URL/open-external) gone.** `browser_view.rs:151-218` → absent.
- **[H] PDF pane entirely gone** — no pdfium, no `.pdf` routing, no viewer/nav/zoom/select. `pdf_view.rs:1-659` → absent.
- **[H] Git-Log lane/branch graph gone** — ASCII `--graph` text only. `git_log/graph.rs:517-667` → `warpui/git.rs:70-95`.
- **[H] Git-Log structured commit rows gone** — one muted text line/commit, no author/date/hover/selection. `git_log/view/log.rs:2065-2152` → `shell.rs:6100-6110`.
- **[H] Git-Log refs left column gone** (Local/Remote/Tags/Worktrees, click-to-filter). `git_log/view/refs.rs:1-126` → absent.
- **[H] Git-Log details column + diff-on-select gone.** `git_log/view/details.rs:1-482` → absent.
- **[H] Git-Log commit context menu + branch prompt gone** (Checkout/Create branch/worktree/Cherry-pick/Revert/Copy hash). `git_log/view/log.rs:2159-2203` → absent.

**Medium**
- **[M] Diff minimap strip gone.** `diff_view.rs:748-790` → absent.
- **[M] Diff hunk nav buttons + counter gone.** `diff_view.rs:459-500` → absent.
- **[M] Diff in-body header w/ rename `-> ` path coloring gone.** `diff_view.rs:427-503` → `warpui/diff_view.rs:255-293`.
- **[M] Diff image rendering gone** — image diffs run TextDiff on bytes. `diff_view.rs:806-862` → absent.
- **[M] Diff scroll model drifted** — manual 2000-row window, no scrollbar/horizontal/offset-jump. `diff_view.rs:540-546` → `warpui/diff_view.rs:271-288`.
- **[M] Markdown mixed-emphasis paragraphs don't wrap** (bold/italic/code blocks → non-wrapping Flex row). `markdown_view.rs:71-88` → `warpui/markdown_view.rs:304-326`.
- **[M] Browser URL normalization gone** (localhost→http, bare word→search). `browser_view.rs:452-521` → absent.
- **[M] Git-Log colored ref pills gone** (HEAD/local/remote/tag categories). `git_log/view/log.rs:1593-1676` → absent.
- **[M] Git-Log fetch-all + refresh controls gone.** `git_log/view/mod.rs:1351-1375` → `shell.rs:6067-6098`.
- **[M] Git-Log filter bar gone** (search/branch/user facets). `git_log/view/log.rs:1712-1994` → absent.
- **[M] Git-Log FS-watcher auto-reload gone** — loads once on toggle. `git_log/refresh.rs:889-936` → `shell.rs:6055-6063`.
- **[M] Git-Log query depth 10,000→300 + structured records dropped.** `git_log/data.rs:301-382` → `warpui/git.rs:70-95`.

**Low**
- **[L] Diff error-surface row gone.** `diff_view.rs:508-529` → absent.
- **[L] Diff add/del tint hues drifted** (theme alpha vs hand-picked constants). `diff_view.rs:19-24` → `warpui/diff_view.rs:47-49`.
- **[L] Markdown in-pane path bar + Load + red error gone.** `markdown_view.rs:7-47` → `warpui/markdown_view.rs:216-236`.
- **[L] Markdown fenced-code styling drifted** (surface panel + theme text vs bare tan). `markdown_view.rs:174-184` → `warpui/markdown_view.rs:402-417`.
- **[L] Markdown inline-code chip palette drifted.** `markdown_view.rs:162-173` → `warpui/markdown_view.rs:348-357`.
- **[L] Markdown heading color drifted** (text_header vs accent blue). `markdown_view.rs:185-197` → `warpui/markdown_view.rs:282-297`.
- **[L] Browser footer memory status bar gone.** `browser_view.rs:224-417` → absent.
- **[L] Git-Log keyboard nav + collapsible/resizable columns gone.** `git_log/view/log.rs:2033-2055` → absent.

---

## 7. Top bar / Status bar / Branch picker
**Medium**
- **[M] Panel-toggle icon no longer reflects open/closed** (static SIDEBAR both). `ui/top.rs:39-62` → `shell.rs:5312,5326`.
- **[M] All top-bar button tooltips gone.** `top.rs:44-86` → `shell.rs:3730-3787`.
- **[M] Active file path on status-bar right gone** (replaced by Ln/Col). `ui/status.rs:100-108,174-203` → `shell.rs:5382-5411`.
- **[M] Branch picker is now a fixed centered modal**, not the bottom-anchored resizable popup with persisted size + drag grip. `branch_picker.rs:25-49` → `shell.rs:2515-2706`.
- **[M] Branch picker dirty-tree warning banner gone.** `branch_picker.rs:142-165` → absent.
- **[M] Multi-repo (monorepo/submodule) filter chips gone** — single active_cwd root only. `branch_picker.rs:404-570` → `shell.rs:3134-3149`.
- **[M] Collapsible Local/per-remote sections + counts gone** (flat list). `branch_picker.rs:446-549` → `shell.rs:2551-2652`.
- **[M] Per-branch current/open/create badges gone** — can't tell which branches already have a worktree. `branch_picker.rs:633-660` → `shell.rs:2553-2589`.
- **[M] Primary-action inverted:** old name-click = switch-to/create worktree (in-place `git switch` was opt-in pill); new name-click = in-place `git checkout`, worktree is opt-in. `branch_picker.rs:488-500` → `shell.rs:2563-2645`.

**Low**
- **[L] Top-bar bottom divider line gone.** `top.rs:24-31` → `shell.rs:5330`.
- **[L] Status-bar top divider line gone.** `status.rs:21-27` → `shell.rs:5419`.
- **[L] Breadcrumb color/size dimmer** (muted 12.0 vs text 12.5). `top.rs:48-52` → `shell.rs:5286-5292`.
- **[L] Breadcrumb separator `" / "` → `"  /  "`.** `state.rs:2816-2821` → `shell.rs:5271`.
- **[L] `ui.separator()` between chrome/split buttons gone.** `top.rs:69` → `shell.rs:5318-5326`.
- **[L] Status branch label color/size dimmer.** `status.rs:37-45` → `shell.rs:5345-5359`.
- **[L] Status branch appends "  -  ready"** (old showed branch name only). `status.rs:40-42` → `shell.rs:5335-5340`.
- **[L] Vertical divider before status chrome buttons gone.** `status.rs:91-100` → `shell.rs:5412-5416`.
- **[L] Help button glyph/tooltip drifted** (KEYBOARD, no tooltip; gear also loses tooltip). `status.rs:76-89` → `shell.rs:5415-5416`.
- **[L] Hover "Switch" in-place pill + tooltip gone.** `branch_picker.rs:662-709` → absent.
- **[L] Inline dismissible checkout-error in picker gone** (routes to global banner + closes). `branch_picker.rs:213-227` → `shell.rs:7517-7519`.
- **[L] Picker title drift** ("Switch Branch" 15, no git-branch icon, no close tooltip). `branch_picker.rs:112-139` → `shell.rs:1959-1979`.
- **[L] Branch-list loading spinner gone** (flag tracked, no UI). `branch_picker.rs:241-251` → `shell.rs:2657-2665`.
- **[L] 150ms open-grace anti-race gone** (generic modal dismissal). `branch_picker.rs:338-354` → `shell.rs:3190-3258`.

---

## 8. Modals & Toasts
**High**
- **[H] OSC 9/777 notification toast + OS banner — absent** (P0 agent-CLI). No queue, no in-app toast, no `notify_rust`, no urgent TTL/breadcrumb/click-to-focus. `modals/notification_toast.rs:1-260` → absent *(↔ §11)*.
- **[H] Confirm-remove-worktree modal — absent.** New dispatches `RemoveWorktree` → immediate `--force`, no unpushed/dirty/is_main warning. `modals/confirm_remove_worktree.rs:1-100` → `shell.rs:1707-1713,7964-7991`.
- **[H] Settings dialog gutted** — 6-section sidebar (Appearance/Editor/Terminal/LSP/Shortcuts/About) → single 420px card (Appearance theme+zoom, About). No font-size slider, no swatches, no mono picker, no syntax-theme override. `modals/settings.rs:70-581` → `shell.rs:2145-2289`.

**Medium**
- **[M] Settings → Language Servers section — absent** (LSP runs but no status/toggle/download UI). `modals/settings_lsp.rs:1-420` → absent.
- **[M] LSP install-prompt modal + download toast — absent.** `modals/lsp_install.rs:1-190` → absent.
- **[M] Missing-project ("Project Not Found" relocate) modal — absent.** `modals/missing_project.rs:1-95` → absent.
- **[M] Find-in-Files simplified** — no regex/case/whole-word/file-mask/scope/preview/context/async streaming/PageUp-Down. `modals/find_in_files.rs:1005-1453` → `shell.rs:2291-2425`.
- **[M] New-Workspace location modes gone** — Global only; no Project-local/Custom/Browse, no branch-locked hint. `modals/new_workspace.rs:600-687` → `shell.rs:2754-2878`.
- **[M] Confirm-close-tab modal — absent** (immediate teardown). `modals/confirm_close_tab.rs:1-77` → `shell.rs:1726-1730,7574`.

**Low**
- **[L] Help/shortcuts content drift** — drops Cmd+~ switcher-prev, Shift+Tab back-tab, Ctrl+C/D, F2/Cmd+R rename. `modals/help.rs:34-64` → `shell.rs:2066-2091`.

*Verify:* Empty-state/welcome (no-project vs no-tab, two buttons) rehomed to `welcome_view.rs` — confirm parity. `modals/empty_state.rs:1-68`.

*(Confirm-delete trash ↔ §3; Tab-switcher release ↔ §9; Update toast ↔ §11.)*

---

## 9. Keyboard shortcuts
**High**
- **[H/verify] Cmd+` / Cmd+~ has no NSEvent OS-level interception.** macOS routes Cmd+` to its native window-cycler before winit; old installed an NSEvent monitor (keyCode 0x32) to swallow it. New handles it purely in `on_keydown` — if warpui's winit windowing is pre-empted, the tab switcher silently breaks on macOS. `mac_keys.rs:250-300` → `shell.rs:6830-6833`. **Needs runtime verification.**

**Medium**
- **[M] Tab-switcher commit-on-Cmd-release gone** — no CMD_HELD tracking; commits only on Enter/click. `modals/tab_switcher.rs`, `mac_keys.rs:60-95` → `shell.rs:3079-3128,6832-6833`.
- **[M] Modal dismissal via Cmd+W gone** — only Esc closes; Cmd+W is swallowed but no-ops for non-typing modals. `shortcuts.rs:56-96` → `shell.rs:6736-6786`.
- **[M] Cmd+Backspace/Delete delete-selected-file gone** — no keyboard delete binding (menu-only). `shortcuts.rs:225-238` → `shell.rs:6787-6873`.
- **[M] Cmd+Z Files-pane file-op undo gone** — only undoes text buffer; file-op stack unported (TODO(parity)). `shortcuts.rs:247-256` → `shell.rs:7258-7269` *(↔ §3)*.
- **[M/verify] Terminal Tab/Shift+Tab NSEvent swallow-gate gone** — now encoded in `input.rs` with no framework-level swallow or terminal-focused guard; parity depends on warpui NOT hijacking Tab for focus nav. `mac_keys.rs:200-248` → `warpui/input.rs:52-61`.

**Low**
- **[L] Cmd+Opt+T browser-new-tab gone** (browser pane unported; chord repurposed to Cmd+Opt+W word-wrap). `shortcuts.rs:123-157` → `shell.rs:6857-6860`.
- **[L] Browser-pane clipboard selectors (copy:/paste:/cut:/selectAll:) gone.** `mac_keys.rs:255-300` → absent.
- **[L] Cmd+F focus git-log filter gone** (unconditional FindFocused). `shortcuts.rs:305-343` → `shell.rs:6820`.
- **[L] Non-Escape action chords (e.g. Cmd+S) now dead inside any modal** (only Esc + zoom pass through). `shortcuts.rs:44-96` → `shell.rs:6758-6785`.

*(Cmd+V image paste ↔ §4.)*

---

## 10. Layout / Panes
**Medium**
- **[M] Inactive-pane dim overlay gone** — the primary focus indicator; now only header tint distinguishes panes. `theme::pane_dim` unused. `pane_view.rs:494-499` → `shell.rs:5578-5584`.
- **[M] Esc-restores-maximized-pane gone** — only the button restores. `pane_view.rs:155-158` → absent.
- **[M] Drop-zone 2px accent border gone** — flat fill only, reads as faint wash. `pane_view.rs:505-515` → `shell.rs:5596-5620`.

**Low**
- **[L] Drop-zone fill hue/alpha drift** (accent@70 vs blue@90, no rounding). `pane_view.rs:508` → `theme.rs:45-48`.
- **[L] Pane header title dropped `N · kind`** — terminals all read bare "Terminal". `pane_view.rs:611` → `shell.rs:5700-5709`.
- **[L] Close button red hover feedback gone** (generic icon_button, no hover). `pane_view.rs:544-546` → `shell.rs:3730-3741`.
- **[L] Maximize button hover bg + PointingHand gone.** `pane_view.rs:565-570` → `shell.rs:5738`.
- **[L] Maximize glyph doesn't toggle** (always ARROWS_OUT). `pane_view.rs:573-578` → `shell.rs:5738`.
- **[L] Maximize tooltip gone.** `pane_view.rs:585` → absent.
- **[L] Splitter 4px→5px.** `pane_view.rs:10` → `split.rs:21`.
- **[L] Split clamp 0.05–0.95 → 0.1–0.9** (panes can't drag as small). `layout.rs:799-803` → `split.rs:83,175`.
- **[L] Pane-header drag cursor Grab/Grabbing → PointingHand/DragCopy** (framework). `pane_view.rs:603-607` → warpui draggable.
- **[L] Focus-transfer on press present-but-worse** — deferred action vs gated consumption; ordering differs. `pane_view.rs:395-408` → `shell.rs:5482-5499`.

*(Browser PaneContent variant ↔ §6.)*

---

## 11. Persistence / Settings / Startup / Chrome
**High**
- **[H] Project-wide filesystem watcher gone** — Changes/diff/tree are TTL poll caches, no auto-refresh on external/agent edits. `file_watcher.rs:1-330` → absent.
- **[H] In-app staged auto-update gone** (download+attach+swap+restart; pkg-manager routing). Only version-string check. `update/apply.rs:1-613` → `warpui/update.rs`.
- **[H] Non-Latin fallback fonts gone** (PingFang/Hiragino/Noto CJK/Arabic/Hebrew/Devanagari) → tofu in editor/terminal/filenames/labels. `startup.rs:199-300` → `shell.rs:540-545,701-702`.

**Medium**
- **[M] Bundled JetBrains Mono + Cascadia (Braille/box-drawing) fallback dropped** — Menlo may tofu TUI block glyphs. `startup.rs:139-172` → `shell.rs:701-702`.
- **[M] Custom monospace font setting gone.** `settings.rs:192-215` → absent.
- **[M] Independent editor/terminal font size gone** — folded into single app zoom. `settings.rs:165-190` → `fontsize.rs:1-61`.
- **[M] Syntax-theme override gone.** `settings.rs:311-340` → absent.
- **[M] Editor prefs (word-wrap / trim-on-save / single-click-open) gone + non-persistent.** `settings.rs:342-357` → `shell.rs:2144-2289`.
- **[M] LSP `language_configs` + `install_prompts_disabled` not loaded/saved** (hardcoded default). `state/settings.rs:43-45` → `shell.rs:961`.
- **[M] Window min-size (800×500) not wired** (TODO). `main.rs:63` → `warpui/mod.rs:130-136`.
- **[M] Window position + maximized/fullscreen not persisted** (only logical size). `main.rs:73` → `persist.rs:155-160`.
- **[M] Native "Check for Updates…" menu item gone** — no manual re-trigger. `platform_menu.rs` → `warpui/platform_menu.rs:101-134`.
- **[M] Git-log panel geometry/selection not persisted.** `session.rs:120-150` → `persist.rs:70-83`.
- **[M] Markdown pane not persisted/restored.** `session.rs:192-194` → `persist.rs:101-167`.
- **[M] Missing-project handling on restore gone** — deleted path not surfaced, no cursor sanitization. `session.rs:377-429` → `projects.rs:379-448`.

**Low**
- **[L] Theme swatches preview + "Open themes folder" gone.** `settings.rs:257-309` → `shell.rs:2149-2196`.
- **[L] Native menu Services/Hide/HideOthers/ShowAll + Window submenu gone.** `platform_menu.rs:70-138` → `warpui/platform_menu.rs`.
- **[L] Native About panel → opens in-app Settings.** `platform_menu.rs:41-50` → `warpui/platform_menu.rs:107-112`.
- **[L] Update prompt states (Dismissed/RemindAt 7-day) persistence gone.** `update/check.rs:18-125` → absent.
- **[L] About GitHub/Releases/Check-updates buttons → plaintext.** `settings.rs:459-490` → `shell.rs:2241-2286`.
- **[L] Multiple terminal-tabs-per-pane + per-tab names not persisted** (single cwd/history). `session.rs:172-225` → `persist.rs:87-134` *(↔ §4)*.
- **[L] Legacy base64 terminal-history migration gone.** `session.rs:657-729` → `persist.rs:87-92`.
- **[L] `commit_message` draft not persisted.** `session.rs:25` → absent.
- **[L] Changes-tree collapse + Files-tree expansion not persisted.** `session.rs:23-24` → absent.
- **[L] Project-group tints + group-collapse not persisted.** `session.rs:45-54` → absent.
- **[L] Rich per-project fields (location mode, custom path, files_skip, last-active) dropped.** `session.rs:79-93` → `projects.rs:390-448`.
- **[L] `apply_style` theme→egui Visuals translation gone** (per-element tokens instead). `startup.rs:303-389` → `theme.rs:1-61`.
- **[L] 2s debounced continuous autosave → event-driven only** — non-action state may not persist. `main.rs:256-327` → `shell.rs:1073-1136`.

*(Browser-pane URL persistence, update toast ↔ §6/§8.)*

---

## By-target-file index (where each cluster lands)

| New file | Fix cluster | Parallel-safe? |
|---|---|---|
| **`src/warpui/shell.rs`** | Projects tree, Changes UI, Files-tree menus/DnD, branch picker, ALL modals, keyboard dispatch, pane headers/dim/maximize, settings card, status/top bar, persistence orchestration, font loading (§540/701) | **SERIAL — the monolith.** Batch all shell.rs edits; expect conflicts. |
| `src/warpui/git.rs` | `-z` parsing, `--untracked-files=all`, hunk stage/unstage, push stdin-null + summary, rename `->`, status-glyph priority, OpStatus repo field, git-log query depth/structured records | Parallel (leaf); thin caller wiring in shell.rs |
| `src/warpui/diff_view.rs` | Syntax highlight, hunk gutter, minimap, hunk nav+counter, in-body header, image diff, scroll model, tints, error row | Parallel |
| `src/warpui/editor_view.rs` | Format-on-save, save banner, clean-buffer reload, syntax-fallback map, incremental highlight, read-only affordances, image/pdf routing, dirty-close confirm, find editable, goto clamp, diag colors, Cmd+hover, LSP did_save | Parallel |
| `src/warpui/gutter_element.rs` | git-diff hover tooltip, current-line band | Parallel |
| `src/warpui/scrollbar_element.rs` | diag + git-change minimap markers; terminal thumb states | Parallel |
| `src/warpui/find_bar_element.rs` | editable field, goto-line glyph | Parallel |
| `src/warpui/markdown_view.rs` | mixed-emphasis wrap, path bar, code/inline/heading palette | Parallel |
| `src/warpui/controller.rs` | xterm-crane terminfo Sync, foreground pgid, TERM_PROGRAM_VERSION, VT_TRACE, malloc relief | Parallel |
| `src/warpui/view.rs` + `grid_element.rs` | clickable paths, copy trim, sub-row wheel, scrollbar visuals, JetBrains Mono, I-beam cursor, image paste | Parallel (both terminal-render files) |
| `src/warpui/input.rs` | Tab/Shift+Tab NSEvent gate (verify) | Parallel |
| `src/warpui/split.rs` | splitter width, clamp | Parallel |
| `src/warpui/theme.rs` | pane_dim usage, drop-zone stroke, apply_style visuals | **Shared token file — semi-serial** (many readers) |
| `src/warpui/persist.rs` + `projects.rs` | git-log/markdown/browser/terminal-tab persistence, missing-project restore, rich project fields, FS-watcher hookpoint | Parallel-ish; couples to shell state structs |
| `src/warpui/update.rs` | staged download/apply/restart, prompt-state persistence | Parallel |
| `src/warpui/platform_menu.rs` | Check-for-Updates, Services/Hide/Window submenu, native About | Parallel |
| `src/warpui/mod.rs` | window min-size + position/maximized persistence | Parallel |
| `src/warpui/fontsize.rs` | split editor/terminal font size from zoom | Parallel |
| **NEW modules to create** | notification toast (OSC 9/777 + notify_rust); **browser pane** (wry); **PDF pane** (pdfium); **git-log graph** module (lanes/refs/details/ctx-menu/filter/watcher); **file-op undo stack** + Files/sidebar **drag-drop**; **FS watcher** (notify); non-Latin **font fallback** loader | Parallel among themselves; each needs a serial shell.rs wiring pass |

### Parallelization guidance
- **Safe to run concurrently (separate files):** every leaf renderer/element file above — `diff_view`, `editor_view`, `markdown_view`, `controller`, `view`, `grid_element`, `input`, `split`, `git`, gutter/scrollbar/find-bar elements, `update`, `platform_menu`, `mod`, `fontsize`, and all NEW modules. Different agents can own these without collision.
- **Must be serial:** anything editing **`shell.rs`** (nearly every gap needs at least an action-enum arm or a `*_card` renderer there). Strategy: land the self-contained logic in leaf/new files in parallel, then do **one serialized shell.rs integration pass** wiring the new actions/cards/menu-items. Treat **`theme.rs`** as semi-serial (token changes ripple).
- **Data-loss items first, regardless of file:** trash-not-rm (`shell.rs:7471`), remove-worktree confirm (`shell.rs:7964`), close-tab confirm (`shell.rs:7574`), dirty-tab confirm (`shell.rs:7320`) — small, high-value, all in shell.rs, so do them in the first serial pass.

*Intentional warpui divergences excluded from the gap count (no action): 80px traffic-light spacer, theme-cycle top-bar pill (new addition), edge-to-edge pane inset (per CLAUDE.md rule), Grab→PointingHand drag cursor (framework limit).*