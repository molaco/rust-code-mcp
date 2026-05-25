//! Semantic code analysis using rust-analyzer

mod loader;
mod position;
mod rename;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use anyhow::Result;

pub(crate) use position::Location;
pub(crate) use rename::RenamePreview;

/// Global semantic service instance (Mutex because AnalysisHost is not Sync)
pub(crate) static SEMANTIC: LazyLock<Mutex<SemanticService>> = LazyLock::new(|| {
    Mutex::new(SemanticService::new())
});

/// Cached project context
struct ProjectContext {
    host: AnalysisHost,
    vfs: Vfs,
    load_kind: LoadKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadKind {
    Fast,
    Full,
}

/// Service for semantic code queries
pub(crate) struct SemanticService {
    projects: HashMap<PathBuf, ProjectContext>,
}

impl SemanticService {
    pub fn new() -> Self {
        Self {
            projects: HashMap::new(),
        }
    }

    /// Get or load project (lazy loading)
    fn get_or_load(&mut self, project_path: &Path) -> Result<()> {
        self.get_or_load_kind(project_path, LoadKind::Fast)
    }

    /// Get or load project with full workspace dependency edges.
    fn get_or_load_full(&mut self, project_path: &Path) -> Result<()> {
        self.get_or_load_kind(project_path, LoadKind::Full)
    }

    fn get_or_load_kind(&mut self, project_path: &Path, requested: LoadKind) -> Result<()> {
        let canonical = project_path.canonicalize()?;

        let needs_load = match self.projects.get(&canonical) {
            Some(ctx) => requested == LoadKind::Full && ctx.load_kind == LoadKind::Fast,
            None => true,
        };

        if needs_load {
            tracing::info!(
                "Loading {:?} IDE for project: {}",
                requested,
                canonical.display()
            );
            let (host, vfs) = match requested {
                LoadKind::Fast => loader::load_project(&canonical)?,
                LoadKind::Full => loader::load_project_full(&canonical)?,
            };
            self.projects.insert(
                canonical,
                ProjectContext {
                    host,
                    vfs,
                    load_kind: requested,
                },
            );
            tracing::info!("IDE loaded successfully");
        }

        Ok(())
    }

    /// Search for symbols by name (for find_definition)
    pub fn symbol_search(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
    ) -> Result<Vec<Location>> {
        self.symbol_search_with_exact(project_path, symbol_name, limit, false)
    }

    /// Search for symbols by name with optional full-name filtering.
    pub fn symbol_search_with_exact(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
        exact: bool,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::symbol_search_with_exact(&ctx.host, &ctx.vfs, symbol_name, limit, exact)
    }

    /// Find all references to symbols matching a name
    /// First finds all symbols matching the name, then finds references for each
    pub fn find_references_by_name(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
    ) -> Result<Vec<Location>> {
        self.find_references_by_name_with_exact(project_path, symbol_name, false)
    }

    /// Find all references to symbols matching a name with optional exact filtering.
    pub fn find_references_by_name_with_exact(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        exact: bool,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::find_references_by_name_with_exact(&ctx.host, &ctx.vfs, symbol_name, exact)
    }

    /// Preview rename of a symbol by name. Does not modify any files.
    pub fn rename_by_name(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        new_name: &str,
    ) -> Result<RenamePreview> {
        self.get_or_load_full(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        rename::rename_by_name(&ctx.host, &ctx.vfs, symbol_name, new_name)
    }

    /// Preview rename of a symbol at a concrete file position. Does not modify any files.
    pub fn rename_by_position(
        &mut self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
        symbol_name: &str,
        new_name: &str,
    ) -> Result<RenamePreview> {
        self.get_or_load_full(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        rename::rename_by_position(
            &ctx.host,
            &ctx.vfs,
            file_path,
            line,
            column,
            symbol_name,
            new_name,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rename_preview_includes_workspace_reverse_dependencies() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let workspace_path = workspace.path();

        write_file(
            &workspace_path.join("Cargo.toml"),
            r#"
[workspace]
members = ["engine_sdk", "engine_consumer"]
resolver = "2"
"#,
        );
        write_file(
            &workspace_path.join("engine_sdk/Cargo.toml"),
            r#"
[package]
name = "engine_sdk"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        );
        let sdk_lib = workspace_path.join("engine_sdk/src/lib.rs");
        write_file(
            &sdk_lib,
            r#"pub trait Engine {
    fn tick(&self);
}
"#,
        );
        write_file(
            &workspace_path.join("engine_consumer/Cargo.toml"),
            r#"
[package]
name = "engine_consumer"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
engine_sdk = { path = "../engine_sdk" }
"#,
        );
        write_file(
            &workspace_path.join("engine_consumer/src/lib.rs"),
            r#"use engine_sdk::Engine;

pub struct Candle;

impl Engine for Candle {
    fn tick(&self) {}
}

pub fn run(engine: &dyn Engine) {
    engine.tick();
}
"#,
        );

        let mut service = SemanticService::new();
        let preview = service
            .rename_by_position(
                workspace_path,
                &sdk_lib,
                1,
                11,
                "Engine",
                "RenamedEngine",
            )
            .expect("rename preview");

        assert!(
            preview
                .edits
                .iter()
                .any(|edit| edit.file_path.ends_with("engine_sdk/src/lib.rs")),
            "expected declaration edit in engine_sdk, got {:?}",
            preview.edits
        );
        assert!(
            preview
                .edits
                .iter()
                .any(|edit| edit.file_path.ends_with("engine_consumer/src/lib.rs")),
            "expected downstream edit in engine_consumer, got {:?}",
            preview.edits
        );
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, contents.trim_start()).expect("write fixture file");
    }
}
