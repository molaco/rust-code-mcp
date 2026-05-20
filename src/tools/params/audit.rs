//! Audit endpoint parameter structs.

use rmcp::schemars;

use super::ListPaginationParams;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UnsafeAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MutStaticAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct MissingDocsAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Optional list of item kinds to audit (e.g. [\"Function\", \"Struct\", \"Enum\", \"Trait\", \"TypeAlias\", \"Const\", \"Static\"]). Default: all 'documentable' kinds (Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, Method) — excludes EnumVariant, AssocConst, AssocType which rarely carry standalone docs.")]
    #[serde(default)]
    pub item_kind: Option<Vec<String>>,
    #[schemars(description = "Drop items inside `::tests::` modules. Default true.")]
    #[serde(default)]
    pub skip_test_items: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct DeriveAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Item kind(s) to audit: any subset of [\"Struct\", \"Enum\", \"Union\"]. Default: all three.")]
    #[serde(default)]
    pub item_kind: Option<Vec<String>>,
    #[schemars(description = "Required derive identifiers (e.g. [\"Debug\"] or [\"Debug\", \"Clone\", \"PartialEq\"]). The canonical recommendation is `Debug` for almost every public type.")]
    pub required_derives: Vec<String>,
    #[schemars(description = "Only audit items whose visibility is `pub` (the §8 'Debug almost always' rule applies to the public surface). Default true.")]
    #[serde(default)]
    pub pub_only: Option<bool>,
    #[schemars(description = "Drop items inside `::tests::` modules. Default true.")]
    #[serde(default)]
    pub skip_test_items: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct RecursionCheckParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Maximum cycle length to detect. Default 5 (covers self-loop + indirect recursion through a few hops). Hard cap: 12.")]
    #[serde(default)]
    pub max_cycle_length: Option<usize>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ChannelCapacityAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Drop findings inside `#[cfg(test)]` modules / fns. Default true.")]
    #[serde(default)]
    pub skip_test_fns: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct FnBodyAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Optional crate qualified name to scope the scan. Default: all local crates.")]
    #[serde(default)]
    pub crate_name: Option<String>,
    #[schemars(description = "Patterns to check. Default: all 8 built-ins. Available: \"unwrap\", \"expect\", \"panic_macros\", \"unwrap_unchecked\", \"transmute\", \"await_in_guard_scope\", \"self_recursion\", \"unbounded_loop\".")]
    #[serde(default)]
    pub patterns: Option<Vec<String>>,
    #[schemars(description = "Drop findings inside `#[cfg(test)]` modules / fns. Default true.")]
    #[serde(default)]
    pub skip_test_fns: Option<bool>,
    #[serde(flatten)]
    pub pagination: ListPaginationParams,
}
