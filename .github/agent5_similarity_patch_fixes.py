from pathlib import Path

corpus = Path("rust/crates/framescope-perceptual/tests/quality_corpus.rs")
text = corpus.read_text()
old = "    for y in 0..16 {\n        for x in 0..16 {"
new = "    for y in 0_usize..16 {\n        for x in 0_usize..16 {"
if text.count(old) != 1:
    raise SystemExit(f"quality corpus loop shape drifted: {text.count(old)}")
corpus.write_text(text.replace(old, new, 1))

store = Path("rust/crates/framescope-similarity-store/src/lib.rs")
text = store.read_text()
old = "    let mut previous_last = None;"
new = "    let mut previous_last: Option<u64> = None;"
if text.count(old) != 1:
    raise SystemExit(f"coverage tracker shape drifted: {text.count(old)}")
store.write_text(text.replace(old, new, 1))
