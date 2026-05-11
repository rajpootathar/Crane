//! Render-time cache for directory listings. The Files Pane used to
//! `read_dir` + sort every visible expanded directory on every frame
//! (one `read_dir` + one `Vec` allocation + one O(n log n) sort per
//! dir per frame). For deeply-expanded projects that's a few ms of
//! pure overhead per frame.
//!
//! This module caches the sorted, filtered listing keyed by
//! (path, mtime). POSIX bumps a directory's mtime when entries are
//! added or removed, so the cache self-invalidates on the changes
//! the tree actually cares about (modifies of file *contents* don't
//! affect the tree). One `stat` + one HashMap probe replaces a
//! whole `read_dir` round-trip on the cache-hit path.
//!
//! No JobSystem involvement — `read_dir` is tens of microseconds for
//! typical project dirs, well below a frame budget. Worker hand-off
//! would add latency, not remove it.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use parking_lot::RwLock;

#[derive(Clone, Debug)]
pub struct DirEntryCached {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

struct CacheEntry {
    mtime: SystemTime,
    entries: Arc<Vec<DirEntryCached>>,
}

#[derive(Default)]
struct Inner {
    /// Bounded; we evict oldest LRU-style when over `MAX_ENTRIES`.
    /// Typical session has tens of expanded dirs, well under the cap.
    map: HashMap<PathBuf, CacheEntry>,
}

const MAX_ENTRIES: usize = 512;

pub struct DirCache {
    inner: RwLock<Inner>,
}

impl DirCache {
    fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Cache-hit path: stat dir for mtime, return Arc clone if it
    /// matches. Cache-miss path: drop read lock, take write lock,
    /// read_dir + sort + insert, return Arc.
    pub fn entries(&self, path: &Path) -> Arc<Vec<DirEntryCached>> {
        let mtime = match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return Arc::new(Vec::new()),
        };

        // Fast path: read lock, mtime matches.
        {
            let inner = self.inner.read();
            if let Some(entry) = inner.map.get(path)
                && entry.mtime == mtime
            {
                return Arc::clone(&entry.entries);
            }
        }

        // Slow path: read + sort + insert.
        let mut listing = match std::fs::read_dir(path) {
            Ok(r) => r
                .filter_map(|e| e.ok())
                .map(|e| {
                    let p = e.path();
                    // Use file_type when possible — avoids a follow-up
                    // stat per entry. Falls back to is_dir() (which
                    // does stat) only if file_type fails.
                    let is_dir = e
                        .file_type()
                        .map(|ft| ft.is_dir())
                        .unwrap_or_else(|_| p.is_dir());
                    DirEntryCached {
                        name: e.file_name().to_string_lossy().into_owned(),
                        path: p,
                        is_dir,
                    }
                })
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        listing.sort_by(|a, b| {
            (!a.is_dir, &a.name).cmp(&(!b.is_dir, &b.name))
        });
        let arc = Arc::new(listing);

        let mut inner = self.inner.write();
        // Bound the map at MAX_ENTRIES. HashMap iteration order is
        // unspecified, so victims are *arbitrary*, not LRU — under a
        // pathological thrash pattern (user expanding 513+ unique
        // dirs on consecutive frames) this could re-evict the
        // hottest entry. In practice expanded-dir count is tens to
        // low hundreds; the cap exists purely to prevent unbounded
        // growth from a long session that explored a huge tree.
        if inner.map.len() >= MAX_ENTRIES {
            let drop_count = inner.map.len() - MAX_ENTRIES + 1;
            let keys: Vec<PathBuf> = inner.map.keys().take(drop_count).cloned().collect();
            for k in keys {
                inner.map.remove(&k);
            }
        }
        inner.map.insert(
            path.to_path_buf(),
            CacheEntry {
                mtime,
                entries: Arc::clone(&arc),
            },
        );
        arc
    }

    /// Drop a specific path from the cache. Called from FileWatcher
    /// drain when a path's parent directory was touched — covers the
    /// rare cases where mtime didn't change between consecutive
    /// frames (e.g. atomic rename on the same filesystem block).
    pub fn invalidate(&self, path: &Path) {
        self.inner.write().map.remove(path);
    }

    pub fn clear(&self) {
        self.inner.write().map.clear();
    }
}

static GLOBAL: OnceLock<Arc<DirCache>> = OnceLock::new();

pub fn global() -> Arc<DirCache> {
    Arc::clone(GLOBAL.get_or_init(|| Arc::new(DirCache::new())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cache_returns_same_arc_when_mtime_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        let cache = DirCache::new();
        let a = cache.entries(dir.path());
        let b = cache.entries(dir.path());
        assert!(Arc::ptr_eq(&a, &b), "cache hit must reuse Arc");
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn cache_invalidates_on_directory_change() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("first.txt"), "x").unwrap();
        let cache = DirCache::new();
        let first = cache.entries(dir.path());
        assert_eq!(first.len(), 1);

        // mtime resolution on macOS HFS+ is 1s; sleep just past that.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(dir.path().join("second.txt"), "y").unwrap();

        let second = cache.entries(dir.path());
        assert_eq!(second.len(), 2, "new file must appear after mtime bump");
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn entries_sorted_dirs_first_then_alpha() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("zdir")).unwrap();
        fs::write(dir.path().join("afile"), "a").unwrap();
        fs::create_dir(dir.path().join("adir")).unwrap();
        fs::write(dir.path().join("zfile"), "z").unwrap();

        let cache = DirCache::new();
        let entries = cache.entries(dir.path());
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["adir", "zdir", "afile", "zfile"]);
    }
}
