use serde::{Deserialize, Serialize};

use crate::graph::ids::NodeId;
use crate::graph::model::Node;

#[derive(Debug, Clone)]
pub struct SkeletonOptions {
    pub crates: Option<Vec<String>>,
    pub include: Vec<String>,
    pub include_docs: bool,
    pub include_attrs: bool,
    pub include_impls: bool,
    pub skip_test_items: bool,
    pub exclude_vendor: bool,
}

impl Default for SkeletonOptions {
    fn default() -> Self {
        Self {
            crates: None,
            include: vec!["pub".to_string(), "pub(crate)".to_string()],
            include_docs: true,
            include_attrs: true,
            include_impls: true,
            skip_test_items: true,
            exclude_vendor: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonOutput {
    pub skeleton_dir: String,
    pub snapshot_id: String,
    pub files: Vec<SkeletonFile>,
    pub total_files: usize,
    pub total_items: usize,
    pub total_bytes: usize,
    pub diagnostics: Vec<SkeletonDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonFile {
    pub crate_name: String,
    pub source_path: String,
    pub skeleton_path: String,
    pub content: String,
    pub bytes: usize,
    pub items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonDiagnostic {
    pub message: String,
}

#[derive(Debug, Clone)]
pub(super) struct CollectedSkeleton {
    pub files: Vec<SkeletonSourceFile>,
    pub diagnostics: Vec<SkeletonDiagnostic>,
}

#[derive(Debug, Clone)]
pub(super) struct SkeletonItem {
    pub id: NodeId,
    pub node: Node,
    pub visibility: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct SkeletonSourceFile {
    pub crate_name: String,
    pub source_path: String,
    pub skeleton_path: String,
    pub items: Vec<SkeletonItem>,
}
