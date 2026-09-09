mod model;
mod progressive;
mod store;

pub use model::{
    FRAME_INDEX_SCHEMA_VERSION, FRAME_TIMELINE_CONTRACT_GENERATION, FrameId, FrameIndexEntry,
    FrameIndexError, FrameIndexLifecycle, FrameIndexOpenDisposition, FrameIndexStatus,
    FrameIndexStreamIdentity, KeyframeAnchor, TimestampSeekSafety,
};
pub use progressive::{
    PROGRESSIVE_INDEX_SCHEMA_VERSION, ProgressiveFrameIndex, ProgressiveIndexError,
    ProgressiveIndexLayer, ProgressiveIndexOpenDisposition, ProgressiveLayerLifecycle,
    ProgressiveLayerStatus, STRUCTURAL_INDEX_GENERATION, SimilarityFingerprintStatus,
    StructuralAnchor, StructuralAnchorKind, VISUAL_INDEX_GENERATION, VisualArtifactKind,
    VisualArtifactMetadata,
};
pub use store::FrameIndex;

#[cfg(test)]
mod tests;
