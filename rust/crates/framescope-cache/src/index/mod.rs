mod model;
mod store;

pub use model::{
    FRAME_INDEX_SCHEMA_VERSION, FRAME_TIMELINE_CONTRACT_GENERATION, FrameId, FrameIndexEntry,
    FrameIndexError, FrameIndexLifecycle, FrameIndexOpenDisposition, FrameIndexStatus,
    FrameIndexStreamIdentity, KeyframeAnchor, TimestampSeekSafety,
};
pub use store::FrameIndex;

#[cfg(test)]
mod tests;
