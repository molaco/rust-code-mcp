//! rust-analyzer semantic analysis APIs for rust-code-mcp.

#![warn(unreachable_pub, dead_code)]

pub mod loader;
pub mod position;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;

pub use position::Location;

/// Cached project context.
struct ProjectContext {
    host: AnalysisHost,
    vfs: Vfs,
}

/// Service for semantic code queries.
pub struct SemanticService {
    projects: HashMap<PathBuf, ProjectContext>,
}

impl SemanticService {
    pub fn new() -> Self {
        Self {
            projects: HashMap::new(),
        }
    }

    /// Get or load project lazily.
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

    /// Search for symbols by name.
    pub fn symbol_search(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self
            .projects
            .get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::symbol_search(&ctx.host, &ctx.vfs, symbol_name, limit)
    }

    /// Find all references to symbols matching a name.
    ///
    /// First finds all symbols matching the name, then finds references for each.
    pub fn find_references_by_name(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self
            .projects
            .get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::find_references_by_name(&ctx.host, &ctx.vfs, symbol_name)
    }
}

impl Default for SemanticService {
    fn default() -> Self {
        Self::new()
    }
}
