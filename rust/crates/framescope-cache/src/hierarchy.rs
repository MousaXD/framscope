use crate::{
    CacheStore, CachedFrame, DiskCacheError, DiskCacheStats, DiskInsertResult, DiskProxyCache,
    FrameCacheKey, ProxyFormat, ProxyFrame, RamCacheStats, RamFrameCache, RamInsertResult,
    SourceIdentity,
};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

/// Result of a navigation-cache lookup.
///
/// A full-resolution RGBA hit and a compressed preview hit are deliberately distinct variants.
/// Callers that require source-quality pixels, such as extraction, must only accept `Full` and
/// otherwise fall back to authoritative source decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLookup {
    Full(CachedFrame),
    Proxy(ProxyFrame),
    Miss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheHierarchyStats {
    pub ram: RamCacheStats,
    pub disk: DiskCacheStats,
}

#[derive(Debug)]
struct SharedRamRegistration {
    desired_budget_bytes: usize,
    explicitly_configured: bool,
    cache: Weak<Mutex<RamFrameCache>>,
}

static SHARED_RAM_CACHES: OnceLock<Mutex<HashMap<PathBuf, SharedRamRegistration>>> = OnceLock::new();

fn shared_ram_registry() -> &'static Mutex<HashMap<PathBuf, SharedRamRegistration>> {
    SHARED_RAM_CACHES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_registry() -> MutexGuard<'static, HashMap<PathBuf, SharedRamRegistration>> {
    shared_ram_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_ram(cache: &Mutex<RamFrameCache>) -> MutexGuard<'_, RamFrameCache> {
    // RAM cache contents are disposable performance state. Recovering the inner value after a panic
    // is safer than turning a poisoned cache mutex into a permanent navigation failure.
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn shared_ram_cache(root: &Path, requested_budget_bytes: usize) -> Arc<Mutex<RamFrameCache>> {
    let root = root.to_path_buf();
    let mut registry = lock_registry();
    registry.retain(|_, entry| entry.explicitly_configured || entry.cache.strong_count() > 0);

    if let Some(entry) = registry.get_mut(&root) {
        if !entry.explicitly_configured && requested_budget_bytes > entry.desired_budget_bytes {
            entry.desired_budget_bytes = requested_budget_bytes;
        }
        if let Some(cache) = entry.cache.upgrade() {
            let desired_budget_bytes = entry.desired_budget_bytes;
            drop(registry);
            let mut ram = lock_ram(&cache);
            if ram.budget_bytes() != desired_budget_bytes {
                ram.set_budget_bytes(desired_budget_bytes);
            }
            drop(ram);
            return cache;
        }

        let cache = Arc::new(Mutex::new(RamFrameCache::new(entry.desired_budget_bytes)));
        entry.cache = Arc::downgrade(&cache);
        return cache;
    }

    let cache = Arc::new(Mutex::new(RamFrameCache::new(requested_budget_bytes)));
    registry.insert(
        root,
        SharedRamRegistration {
            desired_budget_bytes: requested_budget_bytes,
            explicitly_configured: false,
            cache: Arc::downgrade(&cache),
        },
    );
    cache
}

/// Bounded two-tier frame cache used by indexed navigation.
///
/// Full-quality RAM ownership is shared process-wide by cache-root namespace. This lets exact
/// navigation and live scrub reuse the same immutable RGBA allocation instead of maintaining two
/// independent copies. Disk proxy state remains per hierarchy and preserves its existing semantics.
#[derive(Debug)]
pub struct FrameCacheHierarchy {
    ram_root: PathBuf,
    ram: Arc<Mutex<RamFrameCache>>,
    disk: Option<DiskProxyCache>,
    disk_budget_bytes: u64,
    disk_unavailable_reason: Option<String>,
}

impl FrameCacheHierarchy {
    pub fn open(
        disk_root: impl AsRef<Path>,
        ram_budget_bytes: usize,
        disk_budget_bytes: u64,
    ) -> Result<Self, DiskCacheError> {
        let ram_root = disk_root.as_ref().to_path_buf();
        let disk = DiskProxyCache::open(&ram_root, disk_budget_bytes)?;
        Ok(Self {
            ram: shared_ram_cache(&ram_root, ram_budget_bytes),
            ram_root,
            disk: Some(disk),
            disk_budget_bytes,
            disk_unavailable_reason: None,
        })
    }

    pub fn open_resilient(
        disk_root: impl AsRef<Path>,
        ram_budget_bytes: usize,
        disk_budget_bytes: u64,
    ) -> Self {
        let ram_root = disk_root.as_ref().to_path_buf();
        let (disk, disk_unavailable_reason) =
            match DiskProxyCache::open(&ram_root, disk_budget_bytes) {
                Ok(disk) => (Some(disk), None),
                Err(error) => (None, Some(error.to_string())),
            };
        Self {
            ram: shared_ram_cache(&ram_root, ram_budget_bytes),
            ram_root,
            disk,
            disk_budget_bytes,
            disk_unavailable_reason,
        }
    }

    /// Set the persistent desired RAM budget for a cache namespace and synchronously trim any live
    /// hierarchy sharing it. The desired value survives periods with no open hierarchy, so Off and
    /// Custom modes remain effective when the next video session opens.
    pub fn configure_shared_ram_budget(
        disk_root: impl AsRef<Path>,
        ram_budget_bytes: usize,
    ) {
        let root = disk_root.as_ref().to_path_buf();
        let mut registry = lock_registry();
        let entry = registry.entry(root).or_insert_with(|| SharedRamRegistration {
            desired_budget_bytes: ram_budget_bytes,
            explicitly_configured: true,
            cache: Weak::new(),
        });
        entry.desired_budget_bytes = ram_budget_bytes;
        entry.explicitly_configured = true;
        let live = entry.cache.upgrade();
        drop(registry);
        if let Some(cache) = live {
            lock_ram(&cache).set_budget_bytes(ram_budget_bytes);
        }
    }

    pub fn lookup(&mut self, key: &FrameCacheKey) -> Result<CacheLookup, DiskCacheError> {
        if let Some(frame) = lock_ram(&self.ram).get(key) {
            return Ok(CacheLookup::Full(frame));
        }
        let Some(disk) = self.disk.as_mut() else {
            return Err(self.disk_unavailable_error());
        };
        if let Some(proxy) = disk.get(key)? {
            return Ok(CacheLookup::Proxy(proxy));
        }
        Ok(CacheLookup::Miss)
    }

    pub fn lookup_full(&mut self, key: &FrameCacheKey) -> Option<CachedFrame> {
        lock_ram(&self.ram).get(key)
    }

    pub fn insert_full(&mut self, frame: CachedFrame) -> RamInsertResult {
        lock_ram(&self.ram).insert(frame)
    }

    pub fn insert_proxy(
        &mut self,
        key: &FrameCacheKey,
        format: ProxyFormat,
        bytes: &[u8],
    ) -> Result<DiskInsertResult, DiskCacheError> {
        let Some(disk) = self.disk.as_mut() else {
            return Err(self.disk_unavailable_error());
        };
        disk.insert(key, format, bytes)
    }

    pub fn stats(&self) -> CacheHierarchyStats {
        CacheHierarchyStats {
            ram: lock_ram(&self.ram).stats(),
            disk: self
                .disk
                .as_ref()
                .map(DiskProxyCache::stats)
                .unwrap_or_default(),
        }
    }

    pub fn ram_budget_bytes(&self) -> usize {
        lock_ram(&self.ram).budget_bytes()
    }

    /// Applies a new source-quality RAM ceiling synchronously to every hierarchy sharing this root.
    /// Shrinks evict immediately and persist for future sessions in this process.
    pub fn set_ram_budget_bytes(&mut self, ram_budget_bytes: usize) {
        Self::configure_shared_ram_budget(&self.ram_root, ram_budget_bytes);
    }

    pub fn disk_budget_bytes(&self) -> u64 {
        self.disk_budget_bytes
    }

    pub fn disk_available(&self) -> bool {
        self.disk.is_some()
    }

    pub fn disk_unavailable_reason(&self) -> Option<&str> {
        self.disk_unavailable_reason.as_deref()
    }

    fn disk_unavailable_error(&self) -> DiskCacheError {
        DiskCacheError::Io(io::Error::other(
            self.disk_unavailable_reason
                .as_deref()
                .unwrap_or("disk proxy cache is unavailable")
                .to_owned(),
        ))
    }
}

impl CacheStore for FrameCacheHierarchy {
    type Error = DiskCacheError;

    fn invalidate_source(&mut self, source: &SourceIdentity) -> Result<(), Self::Error> {
        lock_ram(&self.ram).invalidate_source(source);
        if let Some(disk) = self.disk.as_mut() {
            disk.invalidate_source(source)?;
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        lock_ram(&self.ram).clear();
        if let Some(disk) = self.disk.as_mut() {
            disk.clear()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameId, OwnedRgbaFrame};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn strong_source(tag: &str) -> SourceIdentity {
        SourceIdentity::new(123, None, Some(tag.to_owned()))
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "framescope-hierarchy-{name}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn rgba_frame(key: FrameCacheKey) -> CachedFrame {
        CachedFrame {
            key,
            pixels: OwnedRgbaFrame::new(2, 2, 8, vec![17; 16]).unwrap(),
        }
    }

    #[test]
    fn lookup_orders_ram_then_disk_then_miss_without_proxy_promotion() {
        let root = temp_root("order");
        let source = strong_source("source-a");
        let key = FrameCacheKey::new(&source, 0, FrameId(7)).unwrap();
        let mut cache = FrameCacheHierarchy::open(&root, 1024, 1024).unwrap();

        assert_eq!(cache.lookup(&key).unwrap(), CacheLookup::Miss);
        let after_miss = cache.stats();
        assert_eq!(after_miss.ram.misses, 1);
        assert_eq!(after_miss.disk.misses, 1);

        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        assert_eq!(
            cache.insert_proxy(&key, ProxyFormat::Jpeg, &jpeg).unwrap(),
            DiskInsertResult::Inserted
        );
        assert!(matches!(cache.lookup(&key).unwrap(), CacheLookup::Proxy(_)));

        assert_eq!(
            cache.insert_full(rgba_frame(key.clone())),
            RamInsertResult::Inserted
        );
        let disk_hits_before = cache.stats().disk.hits;
        assert!(matches!(cache.lookup(&key).unwrap(), CacheLookup::Full(_)));
        assert_eq!(cache.stats().disk.hits, disk_hits_before);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn full_quality_lookup_never_reads_disk_proxy_tier() {
        let root = temp_root("full-only");
        let source = strong_source("source-full");
        let key = FrameCacheKey::new(&source, 0, FrameId(8)).unwrap();
        let mut cache = FrameCacheHierarchy::open(&root, 1024, 1024).unwrap();
        cache
            .insert_proxy(&key, ProxyFormat::Jpeg, &[0xff, 0xd8, 0xff, 0xd9])
            .unwrap();
        let before = cache.stats();
        assert!(cache.lookup_full(&key).is_none());
        assert_eq!(cache.stats().disk.hits, before.disk.hits);
        assert_eq!(cache.stats().disk.misses, before.disk.misses);
        cache.insert_full(rgba_frame(key.clone()));
        assert!(cache.lookup_full(&key).is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hierarchies_with_same_root_share_full_quality_ram_frames() {
        let root = temp_root("shared");
        let source = strong_source("shared-source");
        let key = FrameCacheKey::new(&source, 0, FrameId(4)).unwrap();
        let mut exact = FrameCacheHierarchy::open(&root, 64, 0).unwrap();
        let mut scrub = FrameCacheHierarchy::open(&root, 0, 0).unwrap();

        assert_eq!(exact.insert_full(rgba_frame(key.clone())), RamInsertResult::Inserted);
        assert!(scrub.lookup_full(&key).is_some());
        assert_eq!(scrub.stats().ram.resident_frames, 1);
        assert_eq!(scrub.ram_budget_bytes(), 64);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_ram_shrink_is_immediate_across_shared_hierarchies() {
        let root = temp_root("resize");
        let source = strong_source("source-resize");
        let key_a = FrameCacheKey::new(&source, 0, FrameId(1)).unwrap();
        let key_b = FrameCacheKey::new(&source, 0, FrameId(2)).unwrap();
        let mut first = FrameCacheHierarchy::open(&root, 32, 0).unwrap();
        let second = FrameCacheHierarchy::open(&root, 32, 0).unwrap();
        first.insert_full(rgba_frame(key_a));
        first.insert_full(rgba_frame(key_b));
        assert_eq!(second.stats().ram.resident_bytes, 32);

        first.set_ram_budget_bytes(16);
        assert_eq!(second.ram_budget_bytes(), 16);
        assert_eq!(second.stats().ram.resident_bytes, 16);
        first.set_ram_budget_bytes(0);
        assert_eq!(second.stats().ram.resident_bytes, 0);
        assert_eq!(second.stats().ram.resident_frames, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_budget_persists_until_the_next_hierarchy_opens() {
        let root = temp_root("persist-budget");
        FrameCacheHierarchy::configure_shared_ram_budget(&root, 0);
        let source = strong_source("persist-source");
        let key = FrameCacheKey::new(&source, 0, FrameId(1)).unwrap();
        let mut cache = FrameCacheHierarchy::open(&root, 1024, 0).unwrap();

        assert_eq!(cache.ram_budget_bytes(), 0);
        assert_eq!(cache.insert_full(rgba_frame(key)), RamInsertResult::TooLarge);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_invalidation_clears_both_tiers() {
        let root = temp_root("invalidate");
        let source = strong_source("source-b");
        let key = FrameCacheKey::new(&source, 2, FrameId(3)).unwrap();
        let mut cache = FrameCacheHierarchy::open(&root, 1024, 1024).unwrap();
        cache.insert_full(rgba_frame(key.clone()));
        cache
            .insert_proxy(&key, ProxyFormat::WebP, b"RIFF\x04\x00\x00\x00WEBP")
            .unwrap();
        cache.invalidate_source(&source).unwrap();
        assert_eq!(cache.lookup(&key).unwrap(), CacheLookup::Miss);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clear_keeps_cache_reusable() {
        let root = temp_root("clear");
        let source = strong_source("source-c");
        let key = FrameCacheKey::new(&source, 0, FrameId(1)).unwrap();
        let mut cache = FrameCacheHierarchy::open(&root, 1024, 1024).unwrap();
        cache.insert_full(rgba_frame(key.clone()));
        cache.clear().unwrap();
        assert_eq!(cache.lookup(&key).unwrap(), CacheLookup::Miss);
        assert_eq!(cache.ram_budget_bytes(), 1024);
        assert_eq!(cache.disk_budget_bytes(), 1024);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resilient_open_keeps_ram_usable_when_disk_tier_is_unavailable() {
        let root = temp_root("disk-unavailable");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&root);
        fs::write(&root, b"not-a-directory").unwrap();

        let source = strong_source("source-d");
        let key = FrameCacheKey::new(&source, 0, FrameId(1)).unwrap();
        let missing = FrameCacheKey::new(&source, 0, FrameId(2)).unwrap();
        let mut cache = FrameCacheHierarchy::open_resilient(&root, 1024, 1024);
        assert!(!cache.disk_available());
        assert!(cache.disk_unavailable_reason().is_some());
        assert!(cache.lookup(&missing).is_err());
        cache.insert_full(rgba_frame(key.clone()));
        assert!(matches!(cache.lookup(&key), Ok(CacheLookup::Full(_))));
        cache.clear().unwrap();
        assert!(cache.lookup_full(&key).is_none());
        let _ = fs::remove_file(root);
    }
}
