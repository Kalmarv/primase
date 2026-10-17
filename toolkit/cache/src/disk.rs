//! The cache folder: entries as files, written by rename, read with verification, trimmed by age.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "toolkit I/O: the cache folder the caller names"
)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::{FORMAT, HEADER, Key, Stats, entry, verify};

/// Temporary files older than this are left over from a crashed writer.
const STALE_TEMP: Duration = Duration::from_secs(24 * 60 * 60);

/// A cache directory (see the module docs).
#[derive(Debug)]
pub struct Cache {
    root: PathBuf,
    max_bytes: u64,
    stats: Mutex<Stats>,
    since_trim: AtomicU64,
    temp_counter: AtomicU64,
}

impl Cache {
    /// Use `dir` (created if missing), keeping it under `max_bytes`.
    pub fn open(dir: impl AsRef<Path>, max_bytes: u64) -> io::Result<Self> {
        let root = dir.as_ref().join(format!("v{FORMAT}"));
        fs::create_dir_all(&root)?;
        Ok(Cache {
            root,
            max_bytes,
            stats: Mutex::new(Stats::default()),
            since_trim: AtomicU64::new(0),
            temp_counter: AtomicU64::new(0),
        })
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn count(&self, f: impl FnOnce(&mut Stats)) {
        if let Ok(mut s) = self.stats.lock() {
            f(&mut s);
        }
    }

    fn path(&self, key: &Key) -> PathBuf {
        let hex = key.hex();
        self.root.join(&hex[..2]).join(hex)
    }

    /// The value stored for `key`, verified; `None` on a miss (a corrupt entry is removed and counted).
    pub fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let path = self.path(key);
        let mut bytes = Vec::new();
        let read = fs::File::open(&path).and_then(|mut f| f.read_to_end(&mut bytes));
        if read.is_err() {
            self.count(|s| s.misses += 1);
            return None;
        }
        match verify(key, &bytes) {
            Some(()) => {
                // refresh the entry's age for eviction; a failure only makes it look older
                if let Ok(f) = fs::File::options().write(true).open(&path) {
                    let _ = f.set_modified(SystemTime::now());
                }
                self.count(|s| s.hits += 1);
                bytes.drain(..HEADER);
                Some(bytes)
            }
            None => {
                let _ = fs::remove_file(&path);
                self.count(|s| {
                    s.corrupt += 1;
                    s.misses += 1;
                });
                None
            }
        }
    }

    /// Store `value` for `key`. Another process storing the same key at the same time is not an error.
    pub fn put(&self, key: &Key, value: &[u8]) -> io::Result<()> {
        let path = self.path(key);
        let dir = path.parent().expect("entry path has a folder");
        fs::create_dir_all(dir)?;
        let temp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            self.temp_counter.fetch_add(1, Ordering::Relaxed)
        ));
        let entry = entry(key, value);
        let written = (|| {
            let mut f = fs::File::create(&temp)?;
            f.write_all(&entry)?;
            f.sync_all()
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        if let Err(e) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            // lost a race with a writer of the same key (Windows refuses to replace an open file): fine if that
            // entry is whole
            let theirs = fs::read(&path).ok();
            if !theirs.is_some_and(|b| verify(key, &b).is_some()) {
                return Err(e);
            }
            return Ok(());
        }
        let n = entry.len() as u64;
        self.count(|s| {
            s.written += 1;
            s.written_bytes += n;
        });
        if self.since_trim.fetch_add(n, Ordering::Relaxed) + n > self.max_bytes / 10 {
            self.since_trim.store(0, Ordering::Relaxed);
            let _ = self.trim();
        }
        Ok(())
    }

    /// The cached value for `key`, or `compute`'s, stored. A failure to store is counted, not returned: the value is
    /// still right.
    pub fn get_or_put<E>(
        &self,
        key: &Key,
        compute: impl FnOnce() -> Result<Vec<u8>, E>,
    ) -> Result<Vec<u8>, E> {
        if let Some(v) = self.get(key) {
            return Ok(v);
        }
        let v = compute()?;
        if self.put(key, &v).is_err() {
            self.count(|s| s.write_errors += 1);
        }
        Ok(v)
    }

    /// Evict least recently used entries until the cache is under its cap; remove stale temporary files.
    pub fn trim(&self) -> io::Result<()> {
        let mut entries = Vec::new();
        let mut total = 0u64;
        let now = SystemTime::now();
        for shard in fs::read_dir(&self.root)? {
            let Ok(shard) = shard else { continue };
            let Ok(files) = fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let Ok(meta) = file.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                if file.file_name().to_string_lossy().starts_with(".tmp-") {
                    if now.duration_since(modified).unwrap_or_default() > STALE_TEMP {
                        let _ = fs::remove_file(file.path());
                    }
                    continue;
                }
                total += meta.len();
                entries.push((modified, file.path(), meta.len()));
            }
        }
        if total <= self.max_bytes {
            return Ok(());
        }
        entries.sort();
        for (_, path, len) in entries {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                self.count(|s| {
                    s.evicted += 1;
                    s.evicted_bytes += len;
                });
            }
        }
        Ok(())
    }
}
