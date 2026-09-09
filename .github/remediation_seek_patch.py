from pathlib import Path

path = Path('rust/crates/framescope-video/src/target_rgba_navigation.rs')
text = path.read_text()

old = '''    let target = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::FrameNotIndexed)?;
    let mut decoder = open_checked_decoder(index, open_fresh_decoder)?;

    if let KeyframeAnchor::Keyframe {
        frame_id: anchor_id,
        presentation_timestamp: Some(anchor_timestamp),
    } = target.anchor
    {
        if let Some(timestamp_us) = anchor_timestamp
            .to_microseconds()
            .filter(|value| *value >= 0)
        {
            decoder.seek_for_target_navigation(timestamp_us)?;
            match decode_from_seek(index, &mut decoder, anchor_id, frame_id) {
                Ok((frame, decoded_frames)) => {
                    return Ok((
                        TargetNavigationResult {
                            frame,
                            decoded_frames,
                            used_keyframe_seek: true,
                            fell_back_to_stream_start: false,
                        },
                        decoder,
                    ));
                }
                Err(
                    CachedNavigationError::TimelineMismatch | CachedNavigationError::UnexpectedEof,
                ) => {}
                Err(error) => return Err(error),
            }

            let mut fallback = open_checked_decoder(index, open_fresh_decoder)?;
            let (frame, decoded_frames) = decode_from_start(index, &mut fallback, frame_id)?;
            return Ok((
                TargetNavigationResult {
                    frame,
                    decoded_frames,
                    used_keyframe_seek: true,
                    fell_back_to_stream_start: true,
                },
                fallback,
            ));
        }
    }

    let (frame, decoded_frames) = decode_from_start(index, &mut decoder, frame_id)?;
    Ok((
        TargetNavigationResult {
            frame,
            decoded_frames,
            used_keyframe_seek: false,
            fell_back_to_stream_start: false,
        },
        decoder,
    ))
'''

new = '''    let target = index
        .entry(frame_id)?
        .ok_or(CachedNavigationError::FrameNotIndexed)?;
    let timestamp_seek_safe = index.timestamp_seek_safety()?.permits_timestamp_seek();
    let mut decoder = open_checked_decoder(index, open_fresh_decoder)?;

    if timestamp_seek_safe {
        if let KeyframeAnchor::Keyframe {
            frame_id: anchor_id,
            presentation_timestamp: Some(anchor_timestamp),
        } = target.anchor
        {
            if let Some(timestamp_us) = anchor_timestamp
                .to_microseconds()
                .filter(|value| *value >= 0)
            {
                decoder.seek_for_target_navigation(timestamp_us)?;
                match decode_from_seek(index, &mut decoder, anchor_id, frame_id) {
                    Ok((frame, decoded_frames)) => {
                        return Ok((
                            TargetNavigationResult {
                                frame,
                                decoded_frames,
                                used_keyframe_seek: true,
                                fell_back_to_stream_start: false,
                            },
                            decoder,
                        ));
                    }
                    Err(
                        CachedNavigationError::TimelineMismatch
                        | CachedNavigationError::UnexpectedEof,
                    ) => {}
                    Err(error) => return Err(error),
                }

                let mut fallback = open_checked_decoder(index, open_fresh_decoder)?;
                let (frame, decoded_frames) = decode_from_start(index, &mut fallback, frame_id)?;
                return Ok((
                    TargetNavigationResult {
                        frame,
                        decoded_frames,
                        used_keyframe_seek: true,
                        fell_back_to_stream_start: true,
                    },
                    fallback,
                ));
            }
        }
    }

    // Ambiguous persisted keyframe timestamps are not authoritative FrameId anchors. Preserve the
    // exact stream-start proof used by source-quality cached navigation instead of allowing the
    // preview fast path to bypass the persisted seek-safety contract.
    let (frame, decoded_frames) = decode_from_start(index, &mut decoder, frame_id)?;
    Ok((
        TargetNavigationResult {
            frame,
            decoded_frames,
            used_keyframe_seek: false,
            fell_back_to_stream_start: !timestamp_seek_safe,
        },
        decoder,
    ))
'''

if text.count(old) != 1:
    raise SystemExit('expected navigation block not found exactly once')
text = text.replace(old, new, 1)

helper_old = '''    fn complete_index() -> (PathBuf, FrameIndex) {
        let path = temp_path("index.sqlite");
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(100, None, Some("target-rgba-navigation".into()));
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        index.mark_building().unwrap();
        let ticks = [0, 40, 100, 140, 220];
        let mut anchor_id = 0;
        let mut anchor_ticks = 0;
        let entries = ticks
            .iter()
            .enumerate()
            .map(|(id, ticks)| {
                let keyframe = id == 0 || id == 2;
                if keyframe {
                    anchor_id = id as u64;
                    anchor_ticks = *ticks;
                }
                FrameIndexEntry {
                    frame_id: FrameId(id as u64),
                    presentation_timestamp: Some(timestamp(*ticks)),
                    duration: Some(MediaDuration {
                        ticks: 40,
                        time_base: TimeBase::new(1, 1_000).unwrap(),
                    }),
                    keyframe,
                    corrupt: false,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: FrameId(anchor_id),
                        presentation_timestamp: Some(timestamp(anchor_ticks)),
                    },
                }
            })
            .collect::<Vec<_>>();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        (path, index)
    }
'''

helper_new = '''    fn complete_index_with_timeline(
        ticks: &[i64],
        keyframes: &[usize],
    ) -> (PathBuf, FrameIndex) {
        let path = temp_path("index.sqlite");
        let identity = FrameIndexStreamIdentity::from_stream(&stream()).unwrap();
        let source = SourceIdentity::new(100, None, Some("target-rgba-navigation".into()));
        let (mut index, disposition) = FrameIndex::open_or_create(&path, source, identity).unwrap();
        assert_eq!(disposition, FrameIndexOpenDisposition::Created);
        index.mark_building().unwrap();
        let mut anchor_id = 0;
        let mut anchor_ticks = ticks.first().copied().unwrap_or(0);
        let entries = ticks
            .iter()
            .enumerate()
            .map(|(id, ticks)| {
                let keyframe = keyframes.contains(&id);
                if keyframe {
                    anchor_id = id as u64;
                    anchor_ticks = *ticks;
                }
                FrameIndexEntry {
                    frame_id: FrameId(id as u64),
                    presentation_timestamp: Some(timestamp(*ticks)),
                    duration: Some(MediaDuration {
                        ticks: 40,
                        time_base: TimeBase::new(1, 1_000).unwrap(),
                    }),
                    keyframe,
                    corrupt: false,
                    anchor: KeyframeAnchor::Keyframe {
                        frame_id: FrameId(anchor_id),
                        presentation_timestamp: Some(timestamp(anchor_ticks)),
                    },
                }
            })
            .collect::<Vec<_>>();
        index.append_batch(&entries).unwrap();
        index.mark_complete().unwrap();
        (path, index)
    }

    fn complete_index() -> (PathBuf, FrameIndex) {
        complete_index_with_timeline(&[0, 40, 100, 140, 220], &[0, 2])
    }
'''

if text.count(helper_old) != 1:
    raise SystemExit('expected complete_index helper not found exactly once')
text = text.replace(helper_old, helper_new, 1)

marker = '''    #[test]
    fn cursor_reuses_nearby_forward_decoder_and_resets_on_reversal_or_large_jump() {
'''
test = '''    #[test]
    fn ambiguous_keyframe_pts_force_stream_start_for_target_only_rgba() {
        let (index_path, index) = complete_index_with_timeline(&[0, 40, 0, 40], &[0, 2]);
        assert!(!index.timestamp_seek_safety().unwrap().permits_timestamp_seek());
        let cache_root = temp_path("ambiguous-seek-cache");
        let mut cache = FrameCacheHierarchy::open(&cache_root, 1024, 0).unwrap();
        let decoded = Arc::new(AtomicU64::new(0));
        let materialized = Arc::new(AtomicU64::new(0));
        let frames = vec![
            rgba_frame(0, 0, true),
            rgba_frame(1, 40, false),
            rgba_frame(2, 0, true),
            rgba_frame(3, 40, false),
        ];

        let result = navigate_to_frame_cached_target_only(
            &index,
            &mut cache,
            || {
                Ok(FakeTargetDecoder {
                    stream: stream(),
                    frames: VecDeque::from(frames.clone()),
                    current: None,
                    decoded_counter: decoded.clone(),
                    materialized_counter: materialized.clone(),
                })
            },
            FrameId(2),
        )
        .unwrap();

        assert_eq!(result.frame_id, FrameId(2));
        assert!(!result.used_keyframe_seek);
        assert!(result.fell_back_to_stream_start);
        assert_eq!(result.pixels.pixels(), &[2_u8; 16]);
        assert_eq!(materialized.load(Ordering::Relaxed), 1);

        drop(index);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir_all(cache_root);
    }

'''
if text.count(marker) != 1:
    raise SystemExit('cursor test marker not found exactly once')
text = text.replace(marker, test + marker, 1)
path.write_text(text)
