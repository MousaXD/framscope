//! Persistent media-index primitives for FrameScope.
//!
//! This crate owns source identity and the metadata-only frame index. It intentionally does not
//! store decoded pixel payloads; RAM and disk frame caches remain separate layers.

mod identity;
mod index;

pub use identity::{
    CacheDirectoryLayout, CacheStore, NoopCacheStore, SourceIdentity, SourceVideoIdentity,
};
pub use index::{
    FRAME_INDEX_SCHEMA_VERSION, FrameId, FrameIndex, FrameIndexEntry, FrameIndexError,
    FrameIndexLifecycle, FrameIndexOpenDisposition, FrameIndexStatus, FrameIndexStreamIdentity,
    KeyframeAnchor,
};
