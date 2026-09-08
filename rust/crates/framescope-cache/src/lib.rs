//! Persistent media-index and bounded frame-cache primitives for FrameScope.
//!
//! This crate owns source identity, the metadata-only frame index, the byte-bounded RAM hot cache,
//! and disposable compressed disk proxy storage. Original-quality extraction must still decode from
//! the authoritative source rather than consuming proxy bytes.

mod disk_cache;
mod frame_cache;
mod hierarchy;
mod identity;
mod index;
mod storage_admin;

pub use disk_cache::{
    DiskCacheError, DiskCacheStats, DiskInsertResult, DiskProxyCache, ProxyFormat, ProxyFrame,
};
pub use frame_cache::{
    CachedFrame, FrameCacheError, FrameCacheKey, OwnedRgbaFrame, RamCacheStats, RamFrameCache,
    RamInsertResult,
};
pub use hierarchy::{CacheHierarchyStats, CacheLookup, FrameCacheHierarchy};
pub use identity::{
    CacheDirectoryLayout, CacheStore, NoopCacheStore, SourceIdentity, SourceVideoIdentity,
};
pub use index::{
    FRAME_INDEX_SCHEMA_VERSION, FrameId, FrameIndex, FrameIndexEntry, FrameIndexError,
    FrameIndexLifecycle, FrameIndexOpenDisposition, FrameIndexStatus, FrameIndexStreamIdentity,
    KeyframeAnchor,
};
pub use storage_admin::{
    FRAME_INDEX_NAMESPACE, PREVIEW_PROXY_NAMESPACE, FrameScopeStorageStats, StorageAdmin,
    StorageAdminError, StorageCategoryStats, StorageClearReport, StorageClearScope,
};
