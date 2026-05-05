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
use ra_ap_ide::TryToNav;
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

            // Backfill the Item Node's file/span from the canonical declaration
            // site. Cheap (single call), and makes dead_pub_report findings
            // navigable. Errors/macro-only definitions silently fall through.
            if let Some(nav) = def.try_to_nav(&sema) {
                let target = nav.call_site;
                if let Some(rel) =
                    resolve_workspace_relative(vfs, target.file_id, &workspace_root)
                {
                    if let Some(node) = model.nodes.get_mut(&target_node_id) {
                        node.file = Some(rel);
                        node.span = Some((
                            u32::from(target.full_range.start()),
                            u32::from(target.full_range.end()),
                        ));
                    }
                }
            }

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
                    // Consumer-module attribution stays on the file-root scope
                    // path — pre-Layer-10 behaviour. This keeps refs surfaced
                    // through macro expansion (where the token's parent may
                    // not be a useful ancestor) intact.
                    let Some(scope) = sema.scope_at_offset(syntax, r.range.start()) else {
                        continue;
                    };
                    let consumer_module: ModuleId = scope.module().into();
                    let Some(&consumer_node_id) = module_node_for.get(&consumer_module) else {
                        continue;
                    };
                    // Layer 10 — call-graph attribution. `scope_at_offset`
                    // walks ancestors of the given node to find the
                    // containing definition; the file-root scope above gives
                    // a *module*-level resolver where `containing_function`
                    // is always `None`. Re-do the lookup with the token's
                    // parent so the resolver drills into the enclosing body.
                    // Closures attribute to their parent fn; refs in const
                    // initializers / trait bounds / enum discriminants give
                    // `None` (no enclosing fn).
                    let body_scope_node: Option<ra_ap_syntax::SyntaxNode> =
                        match syntax.token_at_offset(r.range.start()) {
                            ra_ap_syntax::TokenAtOffset::None => None,
                            ra_ap_syntax::TokenAtOffset::Single(t) => t.parent(),
                            ra_ap_syntax::TokenAtOffset::Between(a, b) => {
                                b.parent().or_else(|| a.parent())
                            }
                        };
                    let consumer_function = body_scope_node
                        .as_ref()
                        .and_then(|n| sema.scope_at_offset(n, r.range.start()))
                        .and_then(|s| s.containing_function())
                        .and_then(|f| {
                            let id = ra_ap_hir_def::FunctionId::try_from(f).ok()?;
                            def_to_node.get(&ModuleDefId::FunctionId(id)).copied()
                        });
                    model.usages.push(Usage {
                        target: target_node_id,
                        consumer_module: consumer_node_id,
                        file: rel_path.clone(),
                        start: u32::from(r.range.start()),
                        end: u32::from(r.range.end()),
                        category: classify_category(r.category),
                        consumer_function,
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

#[cfg(test)]
mod tests {
    //! Phase A1 fixture tests: verify that the five reference patterns we
    //! advertise (method call, trait dispatch, generic bound, const read,
    //! macro expansion) are actually captured by `extract_usages` via
    //! rust-analyzer's `Definition::usages` API.
    //!
    //! Strategy: Option B — one synthetic tempdir crate exercising all five
    //! patterns, persisted once and shared across tests via a `OnceLock`
    //! (mirrors `queries.rs::tests::shared_snapshot()`).
    //!
    //! These tests load a *real* cargo workspace through rust-analyzer, so
    //! they pay the full RA load cost on first call (~3-5s release).
    //! Subsequent tests reuse the cached snapshot.
    use crate::model::{NodeKind, Usage};
    use crate::snapshot::{BuildOptions, OpenedSnapshot, build_and_persist, open_current};
    use crate::storage::{GraphEnvOptions, GraphPaths};
    use std::sync::OnceLock;

    /// Source of the synthetic fixture crate. Each `pub fn` below exercises
    /// exactly one of the five reference patterns; the targets they refer to
    /// (`Foo::bar`, `Trait::method`, `Bound`, `K`, `FOO`) are all declared in
    /// this same file so the workspace is self-contained.
    const FIXTURE_LIB_RS: &str = r#"
pub struct Foo;
impl Foo {
    pub fn bar() {}
}

pub trait Trait {
    fn method(&self);
}

pub struct WithTrait;
impl Trait for WithTrait {
    fn method(&self) {}
}

pub trait Bound {}

pub const K: u32 = 42;
pub const FOO: u32 = 99;

pub fn use_method_call() {
    Foo::bar();
}

pub fn use_trait_dispatch<T: Trait>(x: T) {
    x.method();
}

pub fn use_generic_bound<U: Bound + Default>() -> U {
    U::default()
}

pub fn read_const() -> u32 {
    K
}

pub fn macro_use() {
    println!("{}", FOO);
}

pub fn compute() -> u32 { 1 }
pub const K2: u32 = compute();

pub fn outer_with_closure() {
    let _f = || Foo::bar();
    _f();
}

pub fn caller() {
    Foo::bar();
    let _ = read_const();
}
"#;

    const FIXTURE_CARGO_TOML: &str = r#"
[package]
name = "synthetic_crate"
version = "0.1.0"
edition = "2021"

[workspace]

[lib]
path = "src/lib.rs"
"#;

    struct SharedSnap {
        _workspace_td: tempfile::TempDir,
        _data_td: tempfile::TempDir,
        snap: OpenedSnapshot,
    }

    /// Build the synthetic fixture crate, run `build_and_persist`, open the
    /// snapshot. Cached across all tests in this module.
    fn shared_snapshot() -> &'static OpenedSnapshot {
        static CACHE: OnceLock<SharedSnap> = OnceLock::new();
        &CACHE
            .get_or_init(|| {
                // 1. Materialize the synthetic crate in a tempdir.
                let workspace_td = tempfile::tempdir().expect("create workspace tempdir");
                let workspace_path = workspace_td.path();
                std::fs::write(
                    workspace_path.join("Cargo.toml"),
                    FIXTURE_CARGO_TOML.trim_start(),
                )
                .expect("write Cargo.toml");
                std::fs::create_dir_all(workspace_path.join("src")).expect("create src dir");
                std::fs::write(
                    workspace_path.join("src").join("lib.rs"),
                    FIXTURE_LIB_RS.trim_start(),
                )
                .expect("write lib.rs");

                // 2. Build & persist into a separate data dir.
                let data_td = tempfile::tempdir().expect("create data tempdir");
                let opts = BuildOptions {
                    data_dir_override: Some(data_td.path().to_path_buf()),
                    ..Default::default()
                };
                let result = build_and_persist(workspace_path, opts)
                    .expect("build_and_persist on synthetic fixture");

                let paths = GraphPaths::for_workspace_in(data_td.path(), &result.workspace_root);
                let snap = open_current(&paths, GraphEnvOptions::default())
                    .expect("open_current succeeds")
                    .expect("snapshot exists after build_and_persist");

                SharedSnap {
                    _workspace_td: workspace_td,
                    _data_td: data_td,
                    snap,
                }
            })
            .snap
    }

    /// Helper: look up a target by qualified name and return all of its
    /// recorded usages. Panics if the name doesn't resolve (a missing target
    /// is a fixture bug, not an interesting test result).
    fn usages_for(snap: &OpenedSnapshot, qualified_name: &str) -> Vec<Usage> {
        let (id, node) = snap
            .lookup_by_qualified_name(qualified_name)
            .expect("lookup_by_qualified_name failed")
            .unwrap_or_else(|| panic!("fixture target `{qualified_name}` not in graph"));
        assert_eq!(
            node.kind,
            NodeKind::Item,
            "target `{qualified_name}` should be an Item, got {:?}",
            node.kind
        );
        snap.usages_of(id).expect("usages_of failed")
    }

    /// Pattern 1 — method call: `Foo::bar()` referencing an inherent method.
    ///
    /// **CAPTURED as of Layer 4.** `extract_impl_items` walks every inherent
    /// `impl Foo { ... }` block and emits an Item node per assoc fn / const /
    /// type, registering it in `def_to_node` so the usages pass picks it up.
    /// The path-position reference at the call site `Foo::bar()` lands as a
    /// Usage of `Foo::bar` directly (RA's `Definition::usages` routes the
    /// reference to the assoc-fn def, not the parent struct).
    #[test]
    fn pattern1_method_call_captured() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Foo::bar");
        assert!(
            !usages.is_empty(),
            "expected >=1 usage of `Foo::bar` from `use_method_call`, got 0"
        );
        for u in &usages {
            assert!(u.file.contains("lib.rs"), "usage file should be lib.rs, got {}", u.file);
        }
    }

    /// Pattern 2 — trait dispatch: `x.method()` where `x: T, T: Trait`.
    ///
    /// **CAPTURED as of Layer 4.** `extract_impl_items` emits Item nodes for
    /// trait declaration items (`trait Trait { fn method(&self); }`). RA's
    /// `Definition::usages` resolves both `x.method()` dispatch and direct
    /// `Trait::method` calls back to the trait declaration's def, so that
    /// single Item covers both forms.
    #[test]
    fn pattern2_trait_dispatch_captured() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Trait::method");
        assert!(
            !usages.is_empty(),
            "expected >=1 usage of `Trait::method` from `use_trait_dispatch`, got 0"
        );
        for u in &usages {
            assert!(u.file.contains("lib.rs"), "usage file should be lib.rs, got {}", u.file);
        }
    }

    /// Pattern 3 — generic bound: `fn f<U: Bound>()` referencing a trait
    /// in a where clause / bound position.
    #[test]
    fn pattern3_generic_bound_captured() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Bound");
        assert!(
            !usages.is_empty(),
            "expected ≥1 usage of trait `Bound` from `use_generic_bound` bound, got 0"
        );
        for u in &usages {
            assert!(u.file.contains("lib.rs"), "usage file should be lib.rs, got {}", u.file);
        }
    }

    /// Pattern 4 — const read: `fn read_const() -> u32 { K }` — bare const
    /// reference in a function body. Should be classified as `Read`.
    #[test]
    fn pattern4_const_read_captured() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::K");
        assert!(
            !usages.is_empty(),
            "expected ≥1 usage of const `K` from `read_const`, got 0"
        );
        for u in &usages {
            assert!(u.file.contains("lib.rs"), "usage file should be lib.rs, got {}", u.file);
        }
    }

    /// Pattern 5 — macro expansion: `println!("{}", FOO);` — the const `FOO`
    /// is named inside a macro call. Whether RA's `Definition::usages`
    /// surfaces references inside macro inputs is the key question.
    #[test]
    fn pattern5_macro_expansion_captured() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::FOO");
        assert!(
            !usages.is_empty(),
            "expected ≥1 usage of const `FOO` from `macro_use` (println! arg), got 0"
        );
        for u in &usages {
            assert!(u.file.contains("lib.rs"), "usage file should be lib.rs, got {}", u.file);
        }
    }

    /// Pattern 6 — Layer 10 call-graph: every method call site inside a fn
    /// body should land with `consumer_function` set to the enclosing fn's
    /// NodeId. `Foo::bar()` is invoked from `use_method_call`,
    /// `outer_with_closure`, and `caller` — at least one should satisfy
    /// `consumer_function.is_some()`.
    #[test]
    fn pattern6_function_attribution_works() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Foo::bar");
        let with_fn: Vec<_> = usages
            .iter()
            .filter(|u| u.consumer_function.is_some())
            .collect();
        assert!(
            !with_fn.is_empty(),
            "expected >=1 call site with consumer_function set, got 0"
        );
    }

    /// Pattern 7 — closures attribute to the parent fn (RA's default for
    /// `SemanticsScope::containing_function`). The closure body
    /// `|| Foo::bar()` inside `outer_with_closure` should yield a Usage of
    /// `Foo::bar` whose `consumer_function == NodeId(outer_with_closure)`.
    #[test]
    fn pattern7_closure_attributes_to_parent_fn() {
        let snap = shared_snapshot();
        let (outer_id, _) = snap
            .lookup_by_qualified_name("synthetic_crate::outer_with_closure")
            .unwrap()
            .expect("outer_with_closure not in graph");
        let bar_usages = usages_for(snap, "synthetic_crate::Foo::bar");
        assert!(
            bar_usages
                .iter()
                .any(|u| u.consumer_function == Some(outer_id)),
            "expected >=1 Foo::bar usage attributed to outer_with_closure (closure-as-parent-fn rule), got 0"
        );
    }

    /// Pattern 8 — references in const initializers (and other non-fn
    /// scopes) should leave `consumer_function = None`. `compute()` is
    /// referenced from `pub const K2: u32 = compute();`, which lives at
    /// const-scope, so the resulting Usage row must carry None.
    #[test]
    fn pattern8_const_initializer_has_no_caller_fn() {
        let snap = shared_snapshot();
        let usages = usages_for(snap, "synthetic_crate::compute");
        let from_const_init: Vec<_> = usages
            .iter()
            .filter(|u| u.consumer_function.is_none())
            .collect();
        assert!(
            !from_const_init.is_empty(),
            "expected >=1 compute() usage with consumer_function=None (the const K2 initializer), got 0"
        );
    }
}
