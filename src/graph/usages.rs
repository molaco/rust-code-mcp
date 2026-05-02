//! Layer 4 — usage extraction pass.
//!
//! For every local Item, call `Definition::usages(sema).all()` and record
//! every non-import reference. Each reference is attributed to its enclosing
//! module via `Semantics::scope_at_offset` (necessary because multiple inline
//! `mod` blocks can share one file). Imports are filtered out — they're
//! already modeled as `Binding`s with `kind != Declared`.
//!
//! Items in dep crates aren't reachable as "local Items" by construction
//! (we only walk `def_to_node` entries that resolve to `NodeKind::Item`,
//! and Item nodes are only emitted for items declared in workspace-local
//! crates). Reference sites in dep-crate files are filtered out by
//! `module_node_for.get(consumer_module_id)` returning None.
//!
//! Cost on coding-agent: ~1.3 ms / item, ~1.4 s total (1087 items, 5.2k refs).
//! See `examples/spike_usages.rs` for the timing harness.

use std::collections::HashMap;
use std::path::Path;

use ra_ap_hir::{ModuleDef, Semantics, attach_db};
use ra_ap_hir_def::{ModuleDefId, ModuleId};
use ra_ap_ide_db::RootDatabase;
use ra_ap_ide_db::defs::Definition;
use ra_ap_ide_db::search::ReferenceCategory;
use ra_ap_syntax::AstNode;
use ra_ap_vfs::{FileId, Vfs};

use super::ids::NodeId;
use super::model::{ExtractionModel, NodeKind, Usage, UsageCategory};

pub fn extract_usages(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    module_node_for: &HashMap<ModuleId, NodeId>,
) {
    let workspace_root = model.workspace_root.clone();

    attach_db(db, || {
        let sema = Semantics::new(db);

        for (&def_id, &target_node_id) in def_to_node {
            // Only compute usages for local Items. Module / ExternalSymbol nodes
            // also live in `def_to_node` and must be skipped.
            let Some(node) = model.nodes.get(&target_node_id) else {
                continue;
            };
            if node.kind != NodeKind::Item {
                continue;
            }

            // Convert ModuleDefId → Definition. Skip variants we don't model
            // as Items (Module, BuiltinType, Macro) — those were already
            // filtered upstream in bindings.rs::process_entry.
            let def: Definition = match ModuleDef::from(def_id) {
                ModuleDef::Function(f) => Definition::Function(f),
                ModuleDef::Adt(a) => Definition::Adt(a),
                ModuleDef::Trait(t) => Definition::Trait(t),
                ModuleDef::TypeAlias(t) => Definition::TypeAlias(t),
                ModuleDef::Const(c) => Definition::Const(c),
                ModuleDef::Static(s) => Definition::Static(s),
                _ => continue,
            };

            let results = def.usages(&sema).all();
            for (ed_file_id, refs) in &results.references {
                let file_id = ed_file_id.file_id(db);
                // Only retain refs in workspace-local files. Dep-crate files
                // canonicalize outside `workspace_root` and produce None here.
                let rel_path =
                    match resolve_workspace_relative(vfs, file_id, &workspace_root) {
                        Some(p) => p,
                        None => continue,
                    };
                let source = sema.parse(*ed_file_id);
                let syntax = source.syntax();
                for r in refs {
                    if r.category.contains(ReferenceCategory::IMPORT) {
                        continue;
                    }
                    let Some(scope) = sema.scope_at_offset(syntax, r.range.start()) else {
                        continue;
                    };
                    let consumer_module: ModuleId = scope.module().into();
                    let Some(&consumer_node_id) = module_node_for.get(&consumer_module) else {
                        continue;
                    };
                    model.usages.push(Usage {
                        target: target_node_id,
                        consumer_module: consumer_node_id,
                        file: rel_path.clone(),
                        start: u32::from(r.range.start()),
                        end: u32::from(r.range.end()),
                        category: classify_category(r.category),
                    });
                }
            }
        }
    });
}

fn classify_category(c: ReferenceCategory) -> UsageCategory {
    // ReferenceCategory is bitflags. We've already stripped IMPORT before
    // reaching here. Order of preference: Write > Read > Test > Other.
    if c.contains(ReferenceCategory::WRITE) {
        UsageCategory::Write
    } else if c.contains(ReferenceCategory::READ) {
        UsageCategory::Read
    } else if c.contains(ReferenceCategory::TEST) {
        UsageCategory::Test
    } else {
        UsageCategory::Other
    }
}

fn resolve_workspace_relative(vfs: &Vfs, file_id: FileId, workspace_root: &Path) -> Option<String> {
    let vfs_path = vfs.file_path(file_id);
    let abs = vfs_path.as_path()?;
    let abs_pathbuf: std::path::PathBuf = abs.to_path_buf().into();
    abs_pathbuf
        .strip_prefix(workspace_root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
