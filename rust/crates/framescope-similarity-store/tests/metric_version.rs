use framescope_cache::{FrameId, FrameIndexStreamIdentity, SourceIdentity};
use framescope_core::{MediaDuration, MediaTimestamp, TimeBase};
use framescope_perceptual::{HYBRID_SIMILARITY_ALGORITHM_VERSION, HybridSimilarityPolicy};
use framescope_similarity::FrameGroup;
use framescope_similarity_store::{SimilarityStore, SimilarityStoreKey, SimilarityStoreLoad};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "framescope-similarity-metric-version-{}-{nonce}",
        std::process::id()
    ))
}

fn stream() -> FrameIndexStreamIdentity {
    FrameIndexStreamIdentity {
        stream_index: 0,
        codec_id: 27,
        codec_name: "h264".into(),
        time_base: TimeBase::new(1, 1_000).unwrap(),
        width: Some(16),
        height: Some(16),
    }
}

fn group() -> FrameGroup {
    let time_base = TimeBase::new(1, 1_000).unwrap();
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
fn persisted_hybrid_metric_version_mismatch_is_invalidated_instead_of_reused() {
    assert!(HYBRID_SIMILARITY_ALGORITHM_VERSION > 1);
    let root = root();
    let store = SimilarityStore::new(&root);
    let key = SimilarityStoreKey::new_hybrid(
        SourceIdentity::new(1_024, Some(77), Some("metric-version-fixture".into())),
        stream(),
        HybridSimilarityPolicy {
            max_hash_distance: 8,
            minimum_luma_similarity: 9_700,
        },
    )
    .unwrap();
    let mut writer = store.begin(&key).unwrap();
    writer.append(&group()).unwrap();
    writer.finish().unwrap();

    let path = store.path_for(&key);
    let connection = Connection::open(&path).unwrap();
    let key_json: String = connection
        .query_row("SELECT key_json FROM similarity_meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut persisted: Value = serde_json::from_str(&key_json).unwrap();
    persisted["config"]["algorithm_version"] =
        Value::from(HYBRID_SIMILARITY_ALGORITHM_VERSION - 1);
    connection
        .execute(
            "UPDATE similarity_meta SET key_json = ?1 WHERE id = 1",
            params![serde_json::to_string(&persisted).unwrap()],
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
