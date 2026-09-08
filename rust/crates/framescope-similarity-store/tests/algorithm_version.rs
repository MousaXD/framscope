use framescope_cache::{FrameId, FrameIndexStreamIdentity, SourceIdentity};
use framescope_core::{CodecInfo, MediaDuration, MediaKind, MediaTimestamp, StreamInfo, TimeBase};
use framescope_perceptual::HybridSimilarityPolicy;
use framescope_similarity::FrameGroup;
use framescope_similarity_store::{
    SimilarityStore, SimilarityStoreKey, SimilarityStoreLoad,
};
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "framescope-similarity-store-algorithm-version-{nonce}"
    ))
}

fn source() -> SourceIdentity {
    SourceIdentity::new(100, Some(10), Some("metric-version-source".into()))
}

fn stream() -> FrameIndexStreamIdentity {
    FrameIndexStreamIdentity::from_stream(&StreamInfo {
        index: 0,
        media_kind: MediaKind::Video,
        codec: CodecInfo {
            id: 27,
            name: "h264".into(),
            decoder_available: true,
        },
        is_default: true,
        time_base: Some(TimeBase::new(1, 1000).unwrap()),
        duration: None,
        frame_count: None,
        width: Some(1920),
        height: Some(1080),
        pixel_format: Some("yuv420p".into()),
        average_frame_rate: None,
        nominal_frame_rate: None,
        rotation_degrees: None,
    })
    .unwrap()
}

fn one_frame_group() -> FrameGroup {
    let time_base = TimeBase::new(1, 1000).unwrap();
    FrameGroup {
        representative_frame: FrameId(0),
        first_frame: FrameId(0),
        last_frame: FrameId(0),
        frame_count: 1,
        start_timestamp: MediaTimestamp {
            ticks: 0,
            time_base,
        },
        end_timestamp: MediaTimestamp {
            ticks: 0,
            time_base,
        },
        start_duration: Some(MediaDuration {
            ticks: 40,
            time_base,
        }),
        end_duration: Some(MediaDuration {
            ticks: 40,
            time_base,
        }),
        representative_similarity_floor: 10_000,
    }
}

#[test]
fn persisted_hybrid_algorithm_version_mismatch_is_invalidated_for_rebuild() {
    let root = root();
    let store = SimilarityStore::new(&root);
    let key = SimilarityStoreKey::new_hybrid(
        source(),
        stream(),
        HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        },
    )
    .unwrap();

    let mut writer = store.begin(&key).unwrap();
    writer.append(&one_frame_group()).unwrap();
    writer.finish().unwrap();

    let path = store.path_for(&key);
    let connection = Connection::open(&path).unwrap();
    let key_json: String = connection
        .query_row(
            "SELECT key_json FROM similarity_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut persisted: Value = serde_json::from_str(&key_json).unwrap();
    persisted["config"]["algorithm_version"] = Value::from(1_u64);
    connection
        .execute(
            "UPDATE similarity_meta SET key_json = ?1 WHERE id = 1",
            [serde_json::to_string(&persisted).unwrap()],
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        store
            .visit_groups_for_frame_count(&key, 1, |_| {})
            .unwrap(),
        SimilarityStoreLoad::InvalidatedStale
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}
