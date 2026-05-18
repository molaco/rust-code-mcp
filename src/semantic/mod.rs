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

pub use position::Location;
pub use rename::{RenameEdit, RenameFileMove, RenamePreview};

/// Global semantic service instance (Mutex because AnalysisHost is not Sync)
pub static SEMANTIC: LazyLock<Mutex<SemanticService>> = LazyLock::new(|| {
    Mutex::new(SemanticService::new())
});

/// Cached project context
struct ProjectContext {
    host: AnalysisHost,
    vfs: Vfs,
}

/// Service for semantic code queries
pub struct SemanticService {
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
        let canonical = project_path.canonicalize()?;

        if !self.projects.contains_key(&canonical) {
            tracing::info!("Loading IDE for project: {}", canonical.display());
            let (host, vfs) = loader::load_project(&canonical)?;
            self.projects.insert(canonical, ProjectContext { host, vfs });
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
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        rename::rename_by_name(&ctx.host, &ctx.vfs, symbol_name, new_name)
    }
}
