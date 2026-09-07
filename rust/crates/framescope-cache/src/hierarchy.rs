use crate::{
    CacheStore, CachedFrame, DiskCacheError, DiskCacheStats, DiskInsertResult, DiskProxyCache,
    FrameCacheKey, ProxyFormat, ProxyFrame, RamCacheStats, RamFrameCache, RamInsertResult,
    SourceIdentity,
};
use std::path::Path;

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

/// Bounded two-tier frame cache used by indexed navigation.
///
/// Lookup order is always RAM first, then compressed disk proxy storage. The hierarchy never
/// promotes a proxy into `CachedFrame`; decoding proxy bytes back into RGBA is a rendering concern
/// and cannot silently satisfy an original-quality request.
#[derive(Debug)]
pub struct FrameCacheHierarchy {
    ram: RamFrameCache,
    disk: DiskProxyCache,
}

impl FrameCacheHierarchy {
    pub fn open(
        disk_root: impl AsRef<Path>,
        ram_budget_bytes: usize,
        disk_budget_bytes: u64,
    ) -> Result<Self, DiskCacheError> {
        Ok(Self {
            ram: RamFrameCache::new(ram_budget_bytes),
            disk: DiskProxyCache::open(disk_root, disk_budget_bytes)?,
        })
    }

    pub fn lookup(&mut self, key: &FrameCacheKey) -> Result<CacheLookup, DiskCacheError> {
        if let Some(frame) = self.ram.get(key) {
            return Ok(CacheLookup::Full(frame));
        }
        if let Some(proxy) = self.disk.get(key)? {
            return Ok(CacheLookup::Proxy(proxy));
        }
        Ok(CacheLookup::Miss)
    }

    pub fn insert_full(&mut self, frame: CachedFrame) -> RamInsertResult {
        self.ram.insert(frame)
    }

    pub fn insert_proxy(
        &mut self,
        key: &FrameCacheKey,
        format: ProxyFormat,
        bytes: &[u8],
    ) -> Result<DiskInsertResult, DiskCacheError> {
        self.disk.insert(key, format, bytes)
    }

    pub fn stats(&self) -> CacheHierarchyStats {
        CacheHierarchyStats {
            ram: self.ram.stats(),
            disk: self.disk.stats(),
        }
    }

    pub fn ram_budget_bytes(&self) -> usize {
        self.ram.budget_bytes()
    }

    pub fn disk_budget_bytes(&self) -> u64 {
        self.disk.budget_bytes()
    }
}

impl CacheStore for FrameCacheHierarchy {
    type Error = DiskCacheError;

    fn invalidate_source(&mut self, source: &SourceIdentity) -> Result<(), Self::Error> {
        self.ram.invalidate_source(source);
        self.disk.invalidate_source(source)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.ram.clear();
        self.disk.clear()
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
        let proxy = cache.lookup(&key).unwrap();
        assert!(matches!(proxy, CacheLookup::Proxy(_)));
        let after_proxy = cache.stats();
        assert_eq!(after_proxy.ram.misses, 2);
        assert_eq!(after_proxy.disk.hits, 1);

        assert_eq!(cache.insert_full(rgba_frame(key.clone())), RamInsertResult::Inserted);
        let disk_hits_before = cache.stats().disk.hits;
        let full = cache.lookup(&key).unwrap();
        assert!(matches!(full, CacheLookup::Full(_)));
        assert_eq!(cache.stats().disk.hits, disk_hits_before);

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
}
