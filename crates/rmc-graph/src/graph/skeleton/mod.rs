//! Mirrored Rust skeleton rendering.
//!
//! V1 is intentionally item-file based: it buckets retained item declarations
//! by their recorded `Node.file` and emits parseable placeholder files.

mod collect;
mod model;
mod render;
mod source;
#[cfg(test)]
mod test_support;

pub use model::{
    SkeletonDiagnostic, SkeletonFile, SkeletonOptions, SkeletonOutput,
};

use anyhow::Result;

use super::snapshot::OpenedSnapshot;

pub fn render_crate_skeletons(
    snap: &OpenedSnapshot,
    opts: &SkeletonOptions,
) -> Result<SkeletonOutput> {
    let collected = collect::collect_skeleton(snap, opts)?;
    render::render_source_skeleton(snap, opts, collected)
}
