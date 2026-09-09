# Global Similar Frames contract

## Status

This document defines the code-level contract for a future global similar-frame retrieval feature. It is intentionally separate from FrameScope's existing contiguous grouping pipeline.

The existing product capability is a temporal partition of consecutive near-duplicate frames. An `A B A` sequence remains three consecutive runs even when the two `A` frames are visually alike. No current UI or API may describe that temporal grouping as global similar-frame search.

## Non-negotiable authority

Global retrieval must never create a second source of frame identity or timing truth. Every descriptor and cluster member is keyed by the authoritative tuple:

- strong source identity
- selected stream identity
- exact persistent `FrameId`
- descriptor algorithm version

Presentation timestamps remain metadata from the authoritative frame index. Similarity code must not synthesize FrameIds from timestamps, nominal FPS, row order, or candidate rank.

A source/index generation change invalidates every derived descriptor and cluster result that cannot prove the same source, stream and FrameId contract.

## Descriptor store

Global retrieval should use a separate versioned descriptor store rather than overloading contiguous `FrameGroup` rows.

Suggested record shape:

```text
GlobalDescriptorRecord {
    source_identity,
    stream_identity,
    frame_id,
    algorithm_version,
    descriptor_kind,
    normalized_descriptor,
    optional_color_summary,
}
```

Requirements:

1. descriptors are compact and deterministic across supported platforms
2. persistence is transactional and corruption-fail-safe
3. schema and algorithm versions are explicit
4. partial builds are never reported as complete
5. cancellation leaves either a clearly partial rebuildable state or no committed derived result
6. descriptor storage stays bounded and does not retain decoded source-quality RGBA unnecessarily

## Candidate lookup

Global candidate generation must avoid an all-pairs `O(N^2)` scan for normal production use.

A suitable first implementation may use deterministic bucketed binary descriptors, multi-index Hamming buckets, or another exact/reproducible approximate-nearest-neighbor scheme. The choice must provide:

- a documented candidate-recall contract
- deterministic indexing for the same descriptor set
- explicit memory/disk bounds
- cancellation checks during build and query
- instrumentation for candidates examined per query and candidate-recall misses on the labeled corpus

The current 64-bit dHash alone is not sufficient as final similarity authority. Its hard-gate false-negative behavior must remain measurable.

## Final confirmation

Candidate membership must be confirmed by a deterministic color-aware comparison or a later versioned metric with equal or stronger regression coverage.

The final metric must have explicit handling for:

- color/hue differences
- brightness and exposure variation
- alpha semantics
- row padding and stride
- resolution/profile compatibility

No threshold may be retuned solely to make a benchmark green. Threshold changes require labeled-corpus evidence including TP, FP, TN, FN, precision, recall and F1.

## Non-contiguous membership model

Global clusters require their own membership relation. Do not coerce arbitrary non-contiguous members into `FrameGroup { first_frame, last_frame }` ranges.

Suggested model:

```text
GlobalSimilarCluster {
    cluster_id,
    representative_frame_id,
    algorithm_version,
}

GlobalSimilarMember {
    cluster_id,
    frame_id,
    confirmation_score,
}
```

Cluster construction must document how chain drift is prevented. A safe initial rule is representative/exemplar confirmation: every admitted member must satisfy the required similarity contract against the chosen representative, not merely against the immediately preceding admitted member.

## Query and product semantics

The future product should expose global behavior explicitly, for example:

- `Find similar frames`
- `Global near-duplicates`

It must remain distinct from:

- `Consecutive near-duplicates`, which exports one representative per consecutive temporal run

For an `A B A` source, a global query may return both `A` frames as related members. The contiguous grouping feature must still produce three temporal runs.

## Required acceptance before product claim

A global similar-frame feature is not implemented merely by this contract. Before UI claims global retrieval, implementation must provide:

1. descriptor persistence keyed by strong source, stream, FrameId and algorithm version
2. non-`O(N^2)` candidate indexing for production-scale media
3. color-aware final confirmation
4. explicit non-contiguous cluster membership
5. deterministic rebuild/reuse behavior
6. cancellation and bounded-memory tests
7. source replacement and stale-derived-state tests
8. the labeled quality corpus covering exact duplicate, compression changes, brightness/exposure, hue collision, subtitle changes, translation, camera shake, crop, scale, rotation, moving foreground, fades, cuts, animation and non-adjacent `A B A`
9. measured TP/FP/TN/FN, precision, recall and F1
10. integration evidence showing no loss of authoritative FrameId/timestamp semantics
