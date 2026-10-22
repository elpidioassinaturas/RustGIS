//! Native disposable caches. A directory lock serializes publication and eviction;
//! per-entry leases protect readers/builders across processes, including PID reuse.
use std::collections::BinaryHeap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MIB: u64 = 1024 * 1024;
const CANDIDATES: usize = 256;
pub const INDEX_DEFAULT_BYTES: u64 = 2048 * MIB;

pub fn index_budget() -> u64 {
    std::env::var("RUSTGIS_SHP_CACHE_MIB").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(INDEX_DEFAULT_BYTES / MIB).clamp(64, 65536)
        * MIB
}
pub fn index_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = TEST_INDEX_DIR.with(|root| root.borrow().clone()) {
        return root;
    }
    crate::source::external::cache_dir("RUSTGIS_SHP_CACHE", "shp-index").unwrap_or_else(index_fallback_dir)
}

// Test-local routing avoids process-wide environment mutation and interference with
// parallel tests or the user's persistent indexes. Production always uses the path above.
#[cfg(test)]
thread_local! {
    static TEST_INDEX_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn with_index_dir<T>(root: PathBuf, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_INDEX_DIR.with(|root| *root.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TEST_INDEX_DIR.with(|current| current.replace(Some(root))));
    run()
}
/// Separate Unix users even when the system temporary directory is shared.
pub fn index_fallback_dir() -> PathBuf {
    #[cfg(unix)]
    let name = format!("rustgis-shp-index-{}", rustix::process::geteuid().as_raw());
    #[cfg(not(unix))]
    let name = "rustgis-shp-index";
    std::env::temp_dir().join(name)
}

/// Never follow a lease symlink or open a special file, including after a rename race.
fn lease_file(path: &Path, create: bool) -> io::Result<File> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_file()) {
        return Err(io::Error::other("cache lease is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("cache lease is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "cache lease is not private to this user"));
        }
    }
    Ok(file)
}
fn open_lock(path: &Path) -> io::Result<File> {
    lease_file(path, true)
}

/// Resolve once, validate the actual storage, and never use an unstable alias again.
fn validate_directory(root: &Path) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = rustix::process::geteuid().as_raw();
        // Strip trailing slash/dot spellings before lstat: those can otherwise follow
        // a final symlink even though symlink_metadata normally does not follow it.
        let absolute: PathBuf = std::path::absolute(root)?.components().collect();
        let metadata = std::fs::symlink_metadata(&absolute)?;
        if !metadata.file_type().is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "cache root is not private to this user"));
        }
        let resolved = absolute.canonicalize()?;
        for (depth, parent) in resolved.ancestors().enumerate() {
            let metadata = std::fs::symlink_metadata(parent)?;
            let untrusted_owner = metadata.uid() != uid && (depth == 0 || metadata.uid() != 0);
            let writable = metadata.mode() & 0o022 != 0;
            if !metadata.file_type().is_dir() || untrusted_owner || (writable && (depth == 0 || metadata.mode() & 0o1000 == 0)) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "cache path is not trusted"));
            }
        }
        Ok(resolved)
    }
    #[cfg(not(unix))]
    {
        // Preserve the platform's existing directory/junction behavior outside Unix.
        if !std::fs::metadata(root)?.is_dir() {
            return Err(io::Error::other("cache root is not a directory"));
        }
        Ok(root.to_path_buf())
    }
}

#[derive(Debug)]
struct Directory {
    root: PathBuf,
    file: File,
}

/// Shared validation for blocking checkout and nonblocking release paths.
fn directory_file(root: &Path) -> io::Result<Directory> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(root)?;
    let root = validate_directory(root)?;
    let file = open_lock(&root.join(".rustgis-cache-v1.lock"))?;
    Ok(Directory { root, file })
}
fn directory_lock(root: &Path) -> io::Result<Directory> {
    let directory = directory_file(root)?;
    directory.file.lock()?;
    Ok(directory)
}
fn lease_path(path: &Path) -> PathBuf {
    path.with_extension("lease")
}
fn state(file: &mut File) -> io::Result<u8> {
    file.rewind()?;
    let mut value = [0u8];
    file.read_exact(&mut value)?;
    Ok(value[0])
}
fn mark(file: &mut File, value: u8) -> io::Result<()> {
    file.rewind()?;
    file.write_all(&[value])?;
    file.set_len(1)?;
    file.flush()?;
    file.set_modified(SystemTime::now())
}
fn unique() -> String {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
fn regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| {
        if !m.file_type().is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o022 != 0 {
                return false;
            }
        }
        true
    })
}
fn owned(name: &str) -> bool {
    if let Some(key) = name.strip_prefix("rg-index-v1-").and_then(|s| s.strip_suffix(".sqlite")) {
        return key.len() == 33 && key.bytes().enumerate().all(|(i, c)| if i == 16 { c == b'-' } else { c.is_ascii_hexdigit() });
    }
    let id = name.strip_prefix("rg-index-work-v1-").and_then(|s| s.strip_suffix(".sqlite"));
    let Some(id) = id else { return false };
    let mut parts = id.split('-');
    parts.next().is_some_and(|s| s.parse::<u32>().is_ok_and(|p| p > 0))
        && parts.next().is_some_and(|s| s.parse::<u128>().is_ok())
        && parts.next().is_some_and(|s| s.parse::<u64>().is_ok())
        && parts.next().is_none()
}
fn existing_lock(path: &Path) -> io::Result<File> {
    if !regular(path) {
        return Err(io::Error::other("missing regular cache lease"));
    }
    lease_file(path, false)
}

/// Kept after the SQLite connection. The sidecar stays locked for the
/// whole lifetime, rather than just during I/O. No PID-based assumption for new caches.
pub struct Lease {
    path: PathBuf,
    lock: Option<File>,
    temporary: bool,
    ready: bool,
    budget: u64,
}
impl Lease {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn commit(&mut self) -> io::Result<()> {
        if !self.temporary && self.path.metadata()?.len() > self.budget {
            self.temporary = true;
        }
        let root = self.path.parent().ok_or_else(|| io::Error::other("missing cache directory"))?;
        let directory = directory_lock(root)?;
        // SQLite uses disposable build settings. Flush its completed pages before
        // recording readiness, so a power loss cannot publish an unflushed index.
        if !self.temporary {
            File::options().read(true).write(true).open(&self.path)?.sync_all()?;
        }
        let file = self.lock.as_mut().ok_or_else(|| io::Error::other("missing cache lease"))?;
        mark(file, if self.temporary { b'T' } else { b'C' })?;
        if !self.temporary {
            file.sync_all()?;
        }
        // The directory lock closes the downgrade/open/eviction race on Unix as well.
        file.unlock()?;
        file.lock_shared()?;
        self.ready = true;
        drop(directory);
        let _ = maintain(root, self.budget);
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let Some(root) = self.path.parent() else { return };
        // A release during another sweep must not deadlock. That sweep, the next
        // checkout, or the next startup will reclaim any now-unlocked temporary file.
        if self.ready
            && !self.temporary
            && let Some(file) = &self.lock
        {
            let _ = file.set_modified(SystemTime::now());
        }
        self.lock.take();
        let Ok(directory) = directory_file(root) else { return };
        if directory.file.try_lock().is_err() {
            retry_maintenance(root, self.budget);
            return;
        }
        if self.temporary || !self.ready {
            let _ = remove_unpinned(&self.path);
        }
        let _ = prune_locked(&directory.root, self.budget);
    }
}

// Releases racing a sweep need one later pass: the sweep may already have counted
// that entry as pinned. Coalesce roots on one bounded worker, never one thread per drop.
fn retry_maintenance(root: &Path, budget: u64) {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    type Jobs = Arc<(Mutex<BTreeMap<PathBuf, u64>>, Condvar)>;
    static WORKER: OnceLock<Option<Jobs>> = OnceLock::new();
    let jobs = WORKER.get_or_init(|| {
        let jobs: Jobs = Arc::new((Mutex::new(BTreeMap::new()), Condvar::new()));
        let worker = jobs.clone();
        std::thread::Builder::new()
            .name("rustgis-cache-retry".into())
            .spawn(move || {
                loop {
                    let job = {
                        let (queue, wake) = &*worker;
                        let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
                        while queue.is_empty() {
                            queue = wake.wait(queue).unwrap_or_else(|e| e.into_inner());
                        }
                        queue.pop_first()
                    };
                    if let Some((root, budget)) = job
                        && root.is_dir()
                    {
                        let _ = maintain(&root, budget);
                    }
                }
            })
            .ok()
            .map(|_| jobs)
    });
    if let Some(jobs) = jobs {
        let (queue, wake) = &**jobs;
        let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.contains_key(root) || queue.len() < 64 {
            queue.entry(root.to_path_buf()).and_modify(|b| *b = (*b).min(budget)).or_insert(budget);
            wake.notify_one();
        }
    }
}

/// Called under the directory lock; excludes symlinks and active readers/builders.
fn remove_unpinned(path: &Path) -> io::Result<bool> {
    if !regular(path) {
        return Ok(false);
    }
    let sidecar = lease_path(path);
    let lock = existing_lock(&sidecar)?;
    if lock.try_lock().is_err() {
        return Ok(false);
    }
    std::fs::remove_file(path)?;
    drop(lock);
    let _ = std::fs::remove_file(sidecar);
    Ok(true)
}

fn create_locked(path: PathBuf, temporary: bool, budget: u64) -> io::Result<(Lease, File)> {
    let mut lock = open_lock(&lease_path(&path))?;
    lock.try_lock().map_err(io::Error::other)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Establish ownership before creating the payload. A missing or invalid lease
    // never authorizes deleting a similarly named unrelated file during a sweep.
    mark(&mut lock, if temporary { b'T' } else { b'B' })?;
    let file = options.open(&path)?;
    Ok((Lease { path, lock: Some(lock), temporary, ready: false, budget }, file))
}

/// Acquire a ready index before opening SQLite. Only names from this protocol are managed.
pub fn pin_index(root: &Path, key: &str, budget: u64) -> io::Result<Option<Lease>> {
    let directory = directory_lock(root)?;
    let path = index_path(&directory.root, key)?;
    if !regular(&path) {
        return Ok(None);
    }
    let mut lock = open_lock(&lease_path(&path))?;
    if lock.try_lock_shared().is_err() || state(&mut lock).ok() != Some(b'C') {
        return Ok(None);
    }
    lock.set_modified(SystemTime::now())?;
    Ok(Some(Lease { path, lock: Some(lock), temporary: false, ready: true, budget }))
}
fn index_path(root: &Path, key: &str) -> io::Result<PathBuf> {
    if key.len() != 33 || !key.bytes().enumerate().all(|(i, c)| if i == 16 { c == b'-' } else { c.is_ascii_hexdigit() }) {
        return Err(io::Error::other("invalid cache key"));
    }
    Ok(root.join(format!("rg-index-v1-{key}.sqlite")))
}
pub fn reserve_index(root: &Path, key: &str, budget: u64) -> io::Result<Lease> {
    let directory = directory_lock(root)?;
    let path = index_path(&directory.root, key)?;
    // A valid generation pinned by another reader is never replaced. Concurrent
    // builders can use a private temporary index without losing functionality.
    let mut lock = open_lock(&lease_path(&path))?;
    let available = lock.try_lock().is_ok();
    let managed = available && matches!(state(&mut lock).ok(), Some(b'C' | b'B' | b'T'));
    if available && (!path.exists() || (managed && regular(&path) && std::fs::remove_file(&path).is_ok())) {
        drop(lock);
        let (lease, file) = create_locked(path, false, budget)?;
        drop(file);
        return Ok(lease);
    }
    drop(lock);
    let (lease, file) = create_locked(directory.root.join(format!("rg-index-work-v1-{}.sqlite", unique())), true, budget)?;
    drop(file);
    Ok(lease)
}

#[derive(Default, Debug)]
pub struct PruneStats {
    pub removed: u64,
    pub remaining_bytes: u64,
    pub pinned_bytes: u64,
}
pub fn maintain(root: &Path, budget: u64) -> io::Result<PruneStats> {
    let directory = directory_lock(root)?;
    prune_locked(&directory.root, budget)
}
fn prune_locked(root: &Path, budget: u64) -> io::Result<PruneStats> {
    let mut stats = PruneStats::default();
    loop {
        stats.remaining_bytes = 0;
        stats.pinned_bytes = 0;
        // Bounded metadata even if a cache folder contains millions of files.
        let mut oldest = BinaryHeap::new();
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".lease") {
                let payload = root.join(format!("{stem}.sqlite"));
                if owned(&payload.file_name().unwrap_or_default().to_string_lossy())
                    && std::fs::symlink_metadata(&payload).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
                    && let Ok(lock) = open_lock(&entry.path())
                    && lock.try_lock().is_ok()
                {
                    drop(lock);
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            if !owned(&name) {
                continue;
            }
            let path = entry.path();
            let bytes = entry.metadata()?.len();
            let Ok(mut lock) = existing_lock(&lease_path(&path)) else {
                stats.remaining_bytes = stats.remaining_bytes.saturating_add(bytes);
                continue;
            };
            if lock.try_lock().is_err() {
                stats.remaining_bytes = stats.remaining_bytes.saturating_add(bytes);
                stats.pinned_bytes = stats.pinned_bytes.saturating_add(bytes);
                continue;
            }
            let flag = state(&mut lock).ok();
            if !matches!(flag, Some(b'C' | b'B' | b'T')) {
                stats.remaining_bytes = stats.remaining_bytes.saturating_add(bytes);
                continue;
            }
            let temporary = flag != Some(b'C') || !name.starts_with("rg-index-v1-");
            if temporary && std::fs::remove_file(&path).is_ok() {
                drop(lock);
                let _ = std::fs::remove_file(lease_path(&path));
                stats.removed += 1;
                continue;
            }
            stats.remaining_bytes = stats.remaining_bytes.saturating_add(bytes);
            oldest.push((lock.metadata()?.modified().unwrap_or(UNIX_EPOCH), name, bytes));
            if oldest.len() > CANDIDATES {
                oldest.pop();
            }
        }
        if stats.remaining_bytes <= budget {
            break;
        }
        let mut removed = false;
        for (_, name, bytes) in oldest.into_sorted_vec() {
            if stats.remaining_bytes <= budget {
                break;
            }
            if remove_unpinned(&root.join(name)).unwrap_or(false) {
                stats.remaining_bytes = stats.remaining_bytes.saturating_sub(bytes);
                stats.removed += 1;
                removed = true;
            }
        }
        if !removed || stats.remaining_bytes <= budget {
            break;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests;
