// The video adapter intentionally mirrors separate identity, timeline, source, target, storage,
// and cancellation inputs at its correctness boundary. Keep Clippy from forcing those invariants
// into an opaque options bag solely to satisfy the argument-count heuristic.
#[allow(clippy::too_many_arguments)]
#[path = "lib.rs"]
mod implementation;

pub use implementation::*;
pub mod global;
