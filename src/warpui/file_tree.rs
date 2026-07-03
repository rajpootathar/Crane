//! Lazy filesystem tree for the Files pane: given a root dir and the set of
//! expanded dirs, produce the flat list of visible rows (directories first,
//! then files, alphabetical — matching Crane's Files tree order). Only
//! expanded directories are read, so deep trees stay cheap.
//!
//! Directory listings flow through a small module-level mtime cache
//! (ported from the old `src/dir_cache.rs`): the previous implementation
//! called `std::fs::read_dir` + sort for every visible expanded directory
//! on every refresh. POSIX bumps a directory's mtime when entries are
//! added or removed, so caching keyed by (path, mtime) self-invalidates on
//! exactly the changes the tree cares about (content edits don't touch the
//! dir mtime). A cache hit is one `stat` + one map probe instead of a full
//! `read_dir` round-trip.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use parking_lot::RwLock;

/// One visible row in the Files tree.
///
/// `git_status` is intentionally *not* computed here — the walk only knows
/// the filesystem. The shell populates it after the fact by cross-referencing
/// its own changes list, colouring rows by change status. Values mirror the
/// single-char git porcelain codes the shell already carries on `Change`
/// (`'M'` modified, `'A'` added, `'D'` deleted, `'R'` renamed, `'?'`
/// untracked, …). `None` = clean / unknown.
pub struct FileRow {
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub name: String,
    pub path: PathBuf,
    pub git_status: Option<char>,
}

/// Directory / file names hidden from the tree everywhere. Same list the
/// old `src/ui/explorer.rs` walk skips (~line 1293).
const SKIP_NAMES: &[&str] = &[".git", "target", "node_modules", ".DS_Store"];

/// Build the flat visible row list for `root`.
///
/// Back-compat shim: existing callers that don't expose nested Projects can
/// keep calling this two-argument form; it walks with an empty skip set.
pub fn build_rows(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<FileRow> {
    build_rows_with_skip(root, expanded, &HashSet::new())
}

/// Build the flat visible row list for `root`, hiding any directory whose
/// path is in `skip_paths`.
///
/// `skip_paths` is the set of directories that are surfaced as their own
/// top-level Projects (e.g. nested git repos living under a loose,
/// non-git parent). Hiding them here keeps them from appearing twice —
/// once as a Project and once inside this tree. Ported from the old
/// `active_project_files_skip` behaviour in `explorer.rs`.
pub fn build_rows_with_skip(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    skip_paths: &HashSet<PathBuf>,
) -> Vec<FileRow> {
    let mut rows = Vec::new();
    walk(root, 0, expanded, skip_paths, &mut rows);
    rows
}

fn walk(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    skip_paths: &HashSet<PathBuf>,
    rows: &mut Vec<FileRow>,
) {
    if depth > 64 {
        return;
    }
    let entries = cached_entries(dir);
    for e in entries.iter() {
        // Hardcoded noise filter — never surface VCS / build / OS junk.
        if SKIP_NAMES.contains(&e.name.as_str()) {
            continue;
        }
        // Nested-Project skip: this dir is shown as its own Project.
        if skip_paths.contains(&e.path) {
            continue;
        }
        let is_expanded = e.is_dir && expanded.contains(&e.path);
        rows.push(FileRow {
            depth,
            is_dir: e.is_dir,
            expanded: is_expanded,
            name: e.name.clone(),
            path: e.path.clone(),
            git_status: None,
        });
        if is_expanded {
            walk(&e.path, depth + 1, expanded, skip_paths, rows);
        }
    }
}

// ---------------------------------------------------------------------------
// mtime-keyed directory listing cache
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CachedDirEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

struct CacheSlot {
    mtime: SystemTime,
    entries: Arc<Vec<CachedDirEntry>>,
}

/// Bounded to guard a long session that explored a huge tree; typical
/// expanded-dir counts are tens to low hundreds, well under the cap.
const MAX_CACHE_ENTRIES: usize = 512;

static DIR_CACHE: OnceLock<RwLock<HashMap<PathBuf, CacheSlot>>> = OnceLock::new();

fn dir_cache() -> &'static RwLock<HashMap<PathBuf, CacheSlot>> {
    DIR_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Sorted, cached listing for `dir`. Cache hit = one `stat` + map probe;
/// cache miss = `read_dir` + sort + insert. Self-invalidates when the dir's
/// mtime changes (entry added/removed).
fn cached_entries(dir: &Path) -> Arc<Vec<CachedDirEntry>> {
    let mtime = match std::fs::metadata(dir).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return Arc::new(Vec::new()),
    };

    // Fast path: shared read lock, mtime still matches.
    {
        let cache = dir_cache().read();
        if let Some(slot) = cache.get(dir)
            && slot.mtime == mtime
        {
            return Arc::clone(&slot.entries);
        }
    }

    // Slow path: read + sort + insert.
    let mut listing: Vec<CachedDirEntry> = match std::fs::read_dir(dir) {
        Ok(read) => read
            .flatten()
            .map(|e| {
                let path = e.path();
                // Prefer file_type() (no extra stat); fall back to is_dir().
                let is_dir = e
                    .file_type()
                    .map(|ft| ft.is_dir())
                    .unwrap_or_else(|_| path.is_dir());
                CachedDirEntry {
                    name: e.file_name().to_string_lossy().into_owned(),
                    path,
                    is_dir,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    // Directories first, then files; each group alphabetical
    // (case-insensitive) — matches Crane's Files tree order.
    listing.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let arc = Arc::new(listing);

    let mut cache = dir_cache().write();
    // Bound the map. HashMap iteration order is unspecified, so victims are
    // arbitrary (not strict LRU); the cap only prevents unbounded growth.
    if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(dir) {
        let drop_count = cache.len() - MAX_CACHE_ENTRIES + 1;
        let victims: Vec<PathBuf> = cache.keys().take(drop_count).cloned().collect();
        for k in victims {
            cache.remove(&k);
        }
    }
    cache.insert(
        dir.to_path_buf(),
        CacheSlot {
            mtime,
            entries: Arc::clone(&arc),
        },
    );
    arc
}

/// Drop a single directory from the listing cache. For the rare case where a
/// dir's mtime didn't change between refreshes (e.g. an atomic rename onto the
/// same filesystem block).
#[allow(dead_code)]
pub fn invalidate_dir(dir: &Path) {
    dir_cache().write().remove(dir);
}

/// Clear the entire directory listing cache.
#[allow(dead_code)]
pub fn clear_cache() {
    dir_cache().write().clear();
}
