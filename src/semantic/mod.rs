//! Semantic code analysis using rust-analyzer

mod loader;
mod position;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use anyhow::Result;

pub use position::Location;

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

    /// Goto definition at position
    pub fn goto_definition(
        &mut self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::goto_definition(&ctx.host, &ctx.vfs, file_path, line, column)
    }

    /// Find all references at position
    pub fn find_references(
        &mut self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::find_references(&ctx.host, &ctx.vfs, file_path, line, column)
    }

    /// Reload project (call when files change significantly)
    pub fn reload(&mut self, project_path: &Path) -> Result<()> {
        let canonical = project_path.canonicalize()?;

        tracing::info!("Reloading IDE for project: {}", canonical.display());
        let (host, vfs) = loader::load_project(&canonical)?;

        self.projects.insert(canonical, ProjectContext { host, vfs });
        tracing::info!("IDE reloaded successfully");

        Ok(())
    }

    /// Invalidate cached project
    pub fn invalidate(&mut self, project_path: &Path) {
        if let Ok(canonical) = project_path.canonicalize() {
            self.projects.remove(&canonical);
        }
    }
}
