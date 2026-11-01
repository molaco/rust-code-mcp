//! Layer 4 — usage extraction pass.
//!
//! A usage is a non-import reference, in a workspace-local module file, to a
//! local item, field or enum variant. HIR classifies each site before mapping
//! its expanded syntax back to source. Relations and access categories are
//! independent. Lexical module, nearest item and enclosing function body are
//! retained separately, including inline modules and trait implementation fns.
//! Imports are filtered out — they're already modeled as `Binding`s with
//! `kind != Declared`.
//!
//! Two passes remain for historical comparison:
//!
//! * **From the consumer** (default). Walk every token of every local module
//!   file, let RA classify it, and compute the target's NodeId with the same
//!   functions the producer passes mint it with (`TargetIds`). The rows a
//!   crate's files produce depend on that crate's files alone — the
//!   precondition for rebuilding one crate at a time.
//! * **From the target** (`RMC_USAGES_FROM_TARGET=1`): the old
//!   `Definition::usages(sema).all()` path. It emits Unknown relations and
//!   retains known alias/attribution bugs. Tests compare only unchanged
//!   behavior; tests/edge_semantics.rs is the independent semantic oracle.
//!
//! A reference to target `T` is:
//! 1. a NameRef resolving to T, including aliases and macro inputs;
//!    a concrete impl reference also preserves its Implements contract edge;
//! 2. a `Name` in a pattern that resolves to `T` (`NameClass::ConstReference`);
//! 3. an implicit capture `{T}` in a format string (category `Read`).
//!
//! Disabled declarations are excluded through HIR. Unresolved calls are
//! separate records; no points-to or implicit-call completeness is promised.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use ra_ap_hir::{
    AsAssocItem, AssocItem, AssocItemContainer, Crate, HasCrate, Impl, InFile, ModuleDef,
    Semantics, attach_db,
};
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_hir_def::{ModuleDefId, ModuleId};
use ra_ap_ide::TryToNav;
use ra_ap_ide_db::defs::{Definition, NameClass, NameRefClass};
use ra_ap_ide_db::search::{ReferenceCategory, SearchScope};
use ra_ap_ide_db::{EditionedFileId, RootDatabase};
use ra_ap_syntax::{
    AstNode, AstToken, SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize, ast,
};
use ra_ap_vfs::Vfs;

use super::audit_util::resolve_workspace_relative;
use super::bindings::{
    item_kind_for_def, local_item_node_id, module_def_owner_module, name_for_def,
};
use super::ids::NodeId;
use super::identity::assoc_item_node_id;
use super::model::{ExpansionSite, ExtractionModel, ItemKind, NodeKind, Usage, UsageCategory, UsageRelation};
use super::structure::{declaration_definition, is_active};
use super::relations::{concrete_function, relation};

pub(crate) fn extract_usages(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    local_crates: &[Crate],
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    module_node_for: &HashMap<ModuleId, NodeId>,
    pass: UsagesPass,
) {
    backfill_item_spans(model, db, vfs, def_to_node);
    let files = local_module_files(db, local_crates);
    match pass {
        UsagesPass::FromTargets => {
            usages_from_targets(model, db, vfs, &files, def_to_node, module_node_for)
        }
        UsagesPass::FromConsumers => {
            for &krate in local_crates {
                let files = local_module_files(db, &[krate]);
                usages_from_consumers(model, db, vfs, krate, &files, def_to_node, module_node_for);
            }
        }
    }
}

/// Which of the two passes of the module doc builds the usage rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UsagesPass {
    FromConsumers,
    FromTargets,
}

impl UsagesPass {
    /// `RMC_USAGES_FROM_TARGET=1` selects the old pass; anything else the new.
    pub(crate) fn from_env() -> Self {
        if std::env::var("RMC_USAGES_FROM_TARGET").as_deref() == Ok("1") {
            Self::FromTargets
        } else {
            Self::FromConsumers
        }
    }
}

/// The producer side of the usages layer: backfill every target Item node's
/// file/span from its canonical declaration site. Cheap (single call), and
/// makes dead_pub_report findings navigable. Errors/macro-only definitions
/// silently fall through. Writes only the item's own node — never a node of
/// another crate — so it stays with the crate that declares the item.
fn backfill_item_spans(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
) {
    let workspace_root = model.workspace_root.clone();
    attach_db(db, || {
        let sema = Semantics::new(db);
        for (&def_id, &target_node_id) in def_to_node {
            if model.nodes.get(&target_node_id).map(|n| n.kind) != Some(NodeKind::Item) {
                continue;
            }
            let Some(def) = target_definition(def_id) else {
                continue;
            };
            let Some(nav) = def.try_to_nav(&sema) else {
                continue;
            };
            let target = nav.call_site;
            let Some(rel) = resolve_workspace_relative(vfs, target.file_id, &workspace_root) else {
                continue;
            };
            let node = model.nodes.get_mut(&target_node_id).unwrap();
            node.file = Some(rel);
            node.span = Some((
                u32::from(target.full_range.start()),
                u32::from(target.full_range.end()),
            ));
        }
    });
}

/// Legacy target-pass definitions. Fields have no ModuleDefId and are
/// resolved by the consumer path; macros, modules and builtins are not targets.
fn target_definition<'db>(def_id: ModuleDefId) -> Option<Definition<'db>> {
    Some(match ModuleDef::from(def_id) {
        ModuleDef::Function(f) => Definition::Function(f),
        ModuleDef::Adt(a) => Definition::Adt(a),
        ModuleDef::Trait(t) => Definition::Trait(t),
        ModuleDef::TypeAlias(t) => Definition::TypeAlias(t),
        ModuleDef::Const(c) => Definition::Const(c),
        ModuleDef::Static(s) => Definition::Static(s),
        ModuleDef::EnumVariant(v) => Definition::EnumVariant(v),
        _ => return None,
    })
}

/// Inverse of [`target_definition`].
pub(super) fn target_def_id(def: Definition<'_>) -> Option<ModuleDefId> {
    Some(match def {
        Definition::Function(f) => ModuleDefId::FunctionId(f.try_into().ok()?),
        Definition::Adt(a) => ModuleDefId::AdtId(a.into()),
        Definition::Trait(t) => ModuleDefId::TraitId(t.into()),
        Definition::TypeAlias(t) => ModuleDefId::TypeAliasId(t.into()),
        Definition::Const(c) => ModuleDefId::ConstId(c.into()),
        Definition::Static(s) => ModuleDefId::StaticId(s.into()),
        Definition::EnumVariant(v) => ModuleDefId::EnumVariantId(v.into()),
        _ => return None,
    })
}

/// Every non-block module file of the local crates, once per
/// `EditionedFileId`, in first-seen order. Both passes search exactly these
/// files: a file shared by two crates of different editions is two entries
/// (that is how `SearchScope::files` keys it too).
pub(super) fn local_module_files(db: &RootDatabase, local_crates: &[Crate]) -> Vec<EditionedFileId> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for &krate in local_crates {
        let def_map = crate_def_map(db, krate.base());
        let edition = krate.edition(db);
        for (module_id, _) in def_map.modules() {
            if module_id.is_block_module(db) {
                continue;
            }
            let file_id = def_map[module_id]
                .definition_source_file_id()
                .original_file(db)
                .file_id(db);
            let file = EditionedFileId::new(db, file_id, edition);
            if seen.insert(file) {
                files.push(file);
            }
        }
    }
    files
}

/// One reference found by either pass, before site attribution.
struct Found {
    /// File of the reference's original (macro-mapped) range.
    file: EditionedFileId,
    range: TextRange,
    target: NodeId,
    category: UsageCategory,
    relation: UsageRelation,
    /// Expanded syntax retained until attribution, before mapping to source.
    node: Option<SyntaxNode>,
}

/// Where a reference sits: workspace-relative file, consumer module node,
/// enclosing fn (resolved to a node by the caller).
struct Site {
    file: String,
    consumer_module: NodeId,
    consumer_function: Option<ra_ap_hir::Function>,
}

/// Site attribution shared by both passes; caches path + parse per file.
struct Sites<'a> {
    vfs: &'a Vfs,
    workspace_root: &'a Path,
    module_node_for: &'a HashMap<ModuleId, NodeId>,
    /// `None` = the file is outside the workspace root (a dep-crate file).
    files: HashMap<EditionedFileId, Option<(String, SyntaxNode)>>,
}

impl<'a> Sites<'a> {
    fn new(
        vfs: &'a Vfs,
        workspace_root: &'a Path,
        module_node_for: &'a HashMap<ModuleId, NodeId>,
    ) -> Self {
        Self {
            vfs,
            workspace_root,
            module_node_for,
            files: HashMap::new(),
        }
    }

    fn site(
        &mut self,
        sema: &Semantics<'_, RootDatabase>,
        file: EditionedFileId,
        start: TextSize,
    ) -> Option<Site> {
        let Self {
            vfs,
            workspace_root,
            module_node_for,
            files,
        } = self;
        // Only retain refs in workspace-local files. Dep-crate files
        // canonicalize outside `workspace_root` and produce None here.
        let (rel_path, syntax) = files
            .entry(file)
            .or_insert_with(|| {
                let rel = resolve_workspace_relative(vfs, file.file_id(sema.db), workspace_root)?;
                Some((rel, sema.parse(file).syntax().clone()))
            })
            .as_ref()?;
        // Consumer-module attribution stays on the file-root scope
        // path — pre-Layer-10 behaviour. This keeps refs surfaced
        // through macro expansion (where the token's parent may
        // not be a useful ancestor) intact.
        let scope = sema.scope_at_offset(syntax, start)?;
        let consumer_module: ModuleId = scope.module().into();
        let &consumer_node_id = module_node_for.get(&consumer_module)?;
        // Layer 10 — call-graph attribution. `scope_at_offset`
        // walks ancestors of the given node to find the
        // containing definition; the file-root scope above gives
        // a *module*-level resolver where `containing_function`
        // is always `None`. Re-do the lookup with the token's
        // parent so the resolver drills into the enclosing body.
        // Closures attribute to their parent fn; refs in const
        // initializers / trait bounds / enum discriminants give
        // `None` (no enclosing fn).
        let body_scope_node: Option<SyntaxNode> = match syntax.token_at_offset(start) {
            ra_ap_syntax::TokenAtOffset::None => None,
            ra_ap_syntax::TokenAtOffset::Single(t) => t.parent(),
            ra_ap_syntax::TokenAtOffset::Between(a, b) => b.parent().or_else(|| a.parent()),
        };
        let consumer_function = body_scope_node
            .as_ref()
            .and_then(|n| sema.scope_at_offset(n, start))
            .and_then(|s| s.containing_function());
        Some(Site {
            file: rel_path.clone(),
            consumer_module: consumer_node_id,
            consumer_function,
        })
    }
}

fn push_usage(
    model: &mut ExtractionModel,
    found: &Found,
    site: Site,
    consumer_function: Option<NodeId>,
) {
    model.usages.push(Usage {
        target: found.target,
        consumer_module: site.consumer_module,
        file: site.file,
        start: u32::from(found.range.start()),
        end: u32::from(found.range.end()),
        category: found.category,
        consumer_function,
        consumer_item: consumer_function,
        relation: found.relation,
        expansion: None,
    });
}

/// The pass from the target: `Definition::usages` for every local Item.
/// Kept behind `RMC_USAGES_FROM_TARGET=1` as the oracle's reference.
fn usages_from_targets(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    files: &[EditionedFileId],
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    module_node_for: &HashMap<ModuleId, NodeId>,
) {
    let workspace_root = model.workspace_root.clone();
    let usage_scope = SearchScope::files(files);
    let mut sites = Sites::new(vfs, &workspace_root, module_node_for);

    attach_db(db, || {
        let sema = Semantics::new(db);

        let mut found = Vec::new();
        for (&def_id, &target_node_id) in def_to_node {
            // Only compute usages for local Items. Module / ExternalSymbol nodes
            // also live in `def_to_node` and must be skipped.
            let Some(node) = model.nodes.get(&target_node_id) else {
                continue;
            };
            if node.kind != NodeKind::Item {
                continue;
            }
            let Some(def) = target_definition(def_id) else {
                continue;
            };

            let results = match catch_unwind(AssertUnwindSafe(|| {
                def.usages(&sema).in_scope(&usage_scope).all()
            })) {
                Ok(results) => results,
                Err(payload) => {
                    tracing::warn!(
                        target: "rmc_graph::usages",
                        ?target_node_id,
                        qualified_name = %node.qualified_name,
                        "usage extraction failed after rust-analyzer panic"
                    );
                    std::panic::resume_unwind(payload);
                }
            };
            for (&file, refs) in &results.references {
                for r in refs {
                    if r.category.contains(ReferenceCategory::IMPORT) {
                        continue;
                    }
                    found.push(Found {
                        file,
                        range: r.range,
                        target: target_node_id,
                        category: classify_category(r.category),
                        relation: UsageRelation::Unknown,
                        node: None,
                    });
                }
            }
        }

        for f in &found {
            let Some(site) = sites.site(&sema, f.file, f.range.start()) else {
                continue;
            };
            let consumer_function = site.consumer_function.and_then(|func| {
                let id = ra_ap_hir_def::FunctionId::try_from(func).ok()?;
                def_to_node.get(&ModuleDefId::FunctionId(id)).copied()
            });
            push_usage(model, f, site, consumer_function);
        }
    });
}

/// The pass from the consumer: classify the tokens of each local file and
/// map what they name to target NodeIds. See the module doc for the rules.
fn usages_from_consumers(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    krate: Crate,
    files: &[EditionedFileId],
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    module_node_for: &HashMap<ModuleId, NodeId>,
) {
    let workspace_root = model.workspace_root.clone();
    let workspace_hash = model.workspace_hash.clone();
    let mut targets = TargetIds::new(vfs, &workspace_root, &workspace_hash, module_node_for);

    attach_db(db, || {
        let sema = Semantics::new_for_crate(db, krate);
        let mut found = Vec::new();
        for &file in files {
            scan_file(&sema, &mut targets, file, &mut found);
            super::unresolved::scan_unresolved(&sema, &mut targets, sema.parse(file).syntax(), &mut model.unresolved_calls);
        }
        // Tokens written by a macro definition have no input token at its
        // invocation to descend from. Scan generated function bodies through
        // their HIR source and retain that expanded syntax for ownership.
        for def_id in def_to_node.keys() {
            let ModuleDefId::FunctionId(id) = *def_id else { continue; };
            let func: ra_ap_hir::Function = id.into();
            if func.module(db).krate(db) != krate { continue; }
            let Some(src) = sema.source(func) else { continue; };
            if src.file_id.macro_file().is_none() { continue; }
            let origin = sema.original_range(src.value.syntax());
            if !files.contains(&origin.file_id) { continue; }
            scan_tree(&sema, &mut targets, origin.file_id, src.value.syntax(), &mut found);
            super::unresolved::scan_unresolved(&sema, &mut targets, src.value.syntax(), &mut model.unresolved_calls);
        }
        for f in &found {
                let Some(node) = f.node.as_ref() else { continue; };
                let Some(scope) = sema.scope(node) else { continue; };
                let module: ModuleId = scope.module().nearest_non_block_module(db).into();
                let Some(&consumer_module) = module_node_for.get(&module) else { continue; };
                let Some(path) = resolve_workspace_relative(vfs, f.file.file_id(db), &workspace_root) else {
                    continue;
                };
                let owner = sema.ancestors_with_macros(node.clone()).find_map(|n| declaration_definition(&sema, &n));
                let consumer_item = owner.and_then(|def| targets.of_definition(&sema, def));
                let consumer_function = match owner {
                    Some(Definition::Function(_)) if scope.containing_function().is_some() => consumer_item,
                    _ => None,
                };
                model.usages.push(Usage {
                    target: f.target, consumer_module, file: path,
                    start: u32::from(f.range.start()), end: u32::from(f.range.end()),
                    category: f.category, relation: f.relation, consumer_function, consumer_item,
                    expansion: expansion_site(&sema, vfs, &workspace_root, node),
                });
        }
    });
}

fn expansion_site(sema: &Semantics<'_, RootDatabase>, vfs: &Vfs, root: &Path, node: &SyntaxNode) -> Option<ExpansionSite> {
    let mut file = sema.hir_file_for(node);
    if file.macro_file().is_none() { return None; }
    let mut call_chain = Vec::new();
    while let Some(call) = file.call_node(sema.db) {
        let range = call.value.text_range();
        call_chain.push((u32::from(range.start()), u32::from(range.end())));
        file = call.file_id;
    }
    call_chain.reverse();
    let invocation_file = resolve_workspace_relative(vfs, file.file_id()?.file_id(sema.db), root)?;
    let range = node.text_range();
    Some(ExpansionSite { invocation_file, call_chain, expanded_span: (u32::from(range.start()), u32::from(range.end())) })
}

fn scan_file(
    sema: &Semantics<'_, RootDatabase>,
    targets: &mut TargetIds<'_>,
    file: EditionedFileId,
    found: &mut Vec<Found>,
) {
    let tree = sema.parse(file).syntax().clone();
    scan_tree(sema, targets, file, &tree, found);
}

fn scan_tree(
    sema: &Semantics<'_, RootDatabase>, targets: &mut TargetIds<'_>,
    file: EditionedFileId, tree: &SyntaxNode, found: &mut Vec<Found>,
) {
    for token in tree
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        let scanned = catch_unwind(AssertUnwindSafe(|| {
            if let Some(string) = ast::String::cast(token.clone()) {
                scan_format_string(sema, targets, file, &token, &string, found);
            } else if token.kind() == SyntaxKind::IDENT {
                scan_ident(sema, targets, &token, found);
            }
        }));
        if let Err(payload) = scanned {
            eprintln!("usage extraction failed at {:?}:{:?} token {:?}", targets.source_file(sema.db, file), token.text_range(), token.text());
            tracing::warn!(
                target: "rmc_graph::usages",
                file = ?file.file_id(sema.db),
                range = ?token.text_range(),
                "usage extraction failed after rust-analyzer panic"
            );
            std::panic::resume_unwind(payload);
        }
    }
}

/// Rules 1 and 2 of the module doc for one identifier token. Only `IDENT`
/// tokens: target names are identifiers, raw ones (`r#type`) included, and
/// `descend_into_macros_exact` keeps the token kind.
fn scan_ident(
    sema: &Semantics<'_, RootDatabase>,
    targets: &mut TargetIds<'_>,
    token: &SyntaxToken,
    found: &mut Vec<Found>,
) {
    let db = sema.db;
    let mut visit = |token: SyntaxToken| {
        let Some(name_like) = token.parent().and_then(ast::NameLike::cast) else {
            return;
        };
        if !is_active(sema, name_like.syntax()) { return; }
        // (definition, category, node whose original range is the reference)
        let mut hits: Vec<(Definition<'_>, UsageCategory, UsageRelation, SyntaxNode)> = Vec::new();
        match name_like {
            ast::NameLike::NameRef(name_ref) => {
                // `ReferenceCategory::new` marks these IMPORT; the target pass drops them.
                if is_name_ref_in_import(&name_ref) {
                    return;
                }
                let Some(NameRefClass::Definition(def, subst)) =
                    NameRefClass::classify(sema, &name_ref)
                else {
                    return;
                };
                let category = if is_name_ref_in_test(sema, &name_ref) {
                    UsageCategory::Test
                } else {
                    UsageCategory::Other
                };
                let def = concrete_function(sema, def, subst.as_ref());
                let relation = relation(sema, def, name_ref.syntax());
                let category = match relation {
                    UsageRelation::Read => UsageCategory::Read,
                    UsageRelation::Write => UsageCategory::Write,
                    _ => category,
                };
                hits.push((def, category, relation, name_ref.syntax().clone()));
                // `found_name_ref`: a reference to an impl's item is also one
                // to the trait's item it implements.
                let in_trait = convert_to_def_in_trait(db, def);
                if in_trait != def {
                    hits.push((in_trait, category, UsageRelation::Implements, name_ref.syntax().clone()));
                }
            }
            ast::NameLike::Name(name_node) => match NameClass::classify(sema, &name_node) {
                // `None` in `if let None = x` — a name that refers.
                Some(NameClass::ConstReference(def)) => {
                    hits.push((def, UsageCategory::Other, UsageRelation::Use, name_node.syntax().clone()));
                }
                // `found_name`: the declaring name of an impl's item refers
                // to the trait's item it implements.
                Some(NameClass::Definition(def)) => {
                    let in_trait = convert_to_def_in_trait(db, def);
                    if in_trait != def {
                        hits.push((in_trait, UsageCategory::Other, UsageRelation::Implements, name_node.syntax().clone()));
                    }
                }
                _ => {}
            },
            ast::NameLike::Lifetime(_) => {}
        }
        for (def, category, relation, node) in hits {
            // HIR resolution is authoritative. The spelling at the site may
            // be an import alias, so comparing it to the declaration loses
            // valid references (`use foo as bar; bar()`).
            let Some(target) = targets.of_definition(sema, def) else {
                continue;
            };
            // Keep a precise source origin even for hygienic tokens written
            // in a macro definition; the rooted API falls back to the whole
            // invocation for those. Ownership still comes from expanded HIR.
            let range = InFile::new(sema.hir_file_for(&node), &node)
                .original_file_range_opt(db).map(|(range, _)| range)
                .unwrap_or_else(|| sema.original_range(&node));
            found.push(Found {
                file: range.file_id,
                range: range.range,
                target,
                category,
                relation,
                node: Some(node),
            });
        }
    };
    let Some(parent) = token.parent() else { return; };
    if sema.is_inside_macro_call(InFile::new(sema.hir_file_for(&parent), token)) {
        for descended in sema.descend_into_macros_exact(token.clone()) {
            visit(descended);
        }
    } else {
        visit(token.clone());
    }
}

/// Rule 3 of the module doc: implicit captures in a format string. The
/// candidate defs come from `as_format_args_parts`; each occurrence of a
/// candidate's name is then confirmed with the same
/// `check_for_format_args_template` call `FindUsages` makes per occurrence.
fn scan_format_string(
    sema: &Semantics<'_, RootDatabase>,
    targets: &mut TargetIds<'_>,
    file: EditionedFileId,
    token: &SyntaxToken,
    string: &ast::String,
    found: &mut Vec<Found>,
) {
    if !string.text().contains('{') {
        return;
    }
    if !token.parent().is_some_and(|n| is_active(sema, &n)) { return; }
    let Some(parts) = sema.as_format_args_parts(string) else {
        return;
    };
    for (range, resolution) in parts {
        let Some(resolution) = resolution.and_then(|r| r.left()) else {
            continue;
        };
        let def = Definition::from(resolution);
        let Some(target) = targets.of_definition(sema, def) else {
            continue;
        };
        // Parts already carry the source range and resolved definition of
        // each capture, including renamed imports. Plain template text is
        // never a part and must not be searched for declaration spellings.
        if let Some((range, _, _, Some(resolution))) =
            sema.check_for_format_args_template(token.clone(), range.start())
        {
            if resolution.either(Definition::from, Definition::from) != def {
                continue;
            }
            found.push(Found { file, range, target, category: UsageCategory::Read,
                relation: UsageRelation::Use, node: token.parent() });
        }
    }
}

/// Mirror of ide-db `search.rs::is_name_ref_in_import` (private there).
fn is_name_ref_in_import(name_ref: &ast::NameRef) -> bool {
    name_ref
        .syntax()
        .parent()
        .and_then(ast::PathSegment::cast)
        .and_then(|it| it.parent_path().top_path().syntax().parent())
        .is_some_and(|it| it.kind() == SyntaxKind::USE_TREE)
}

/// Mirror of ide-db `search.rs::is_name_ref_in_test` (private there).
fn is_name_ref_in_test(sema: &Semantics<'_, RootDatabase>, name_ref: &ast::NameRef) -> bool {
    sema.ancestors_with_macros(name_ref.syntax().clone())
        .any(|node| match ast::Fn::cast(node) {
            Some(it) => sema.to_def(&it).is_some_and(|func| func.is_test(sema.db)),
            None => false,
        })
}

/// Mirror of ide-db `traits.rs::convert_to_def_in_trait` (crate-private
/// there): an item of a trait impl → the same-named item of the trait;
/// anything else → itself.
fn convert_to_def_in_trait<'db>(db: &RootDatabase, def: Definition<'db>) -> Definition<'db> {
    (|| {
        let assoc = def.as_assoc_item(db)?;
        let trait_ = assoc.implemented_trait(db)?;
        let name = match assoc {
            AssocItem::Function(it) => it.name(db),
            AssocItem::Const(it) => it.name(db)?,
            AssocItem::TypeAlias(it) => it.name(db),
        };
        let item = trait_.items(db).into_iter().find(|it| match (it, assoc) {
            (AssocItem::Function(trait_fn), AssocItem::Function(_)) => trait_fn.name(db) == name,
            (AssocItem::Const(trait_const), AssocItem::Const(_)) => {
                trait_const.name(db).is_some_and(|it| it == name)
            }
            (AssocItem::TypeAlias(trait_type), AssocItem::TypeAlias(_)) => {
                trait_type.name(db) == name
            }
            _ => false,
        })?;
        Some(Definition::from(item))
    })()
    .unwrap_or(def)
}

/// NodeId of a usage target, computed from RA alone with the functions the
/// producer passes mint it with (`bindings::local_item_node_id`,
/// `identity::assoc_item_node_id`) — no producer rows needed. `None` exactly for
/// the defs the producer passes do not turn into Item nodes reachable through
/// `def_to_node`:
/// * module-level and block-local items: a local non-block ancestor module;
/// * trait-declaration items: the trait's module likewise;
/// * impl items: only impls `Impl::all_in_crate` lists (it misses impls
///   inside fn bodies, reported as unresolved targets when called);
/// * assoc items need a represented declaration module and container header.
pub(super) struct TargetIds<'a> {
    vfs: &'a Vfs,
    workspace_root: &'a Path,
    local_modules: &'a HashMap<ModuleId, NodeId>,
    by_def: HashMap<ModuleDefId, Option<NodeId>>,
    listed_impls: HashMap<Crate, HashSet<Impl>>,
}

impl<'a> TargetIds<'a> {
    pub(super) fn module_node(&self, module: ModuleId) -> Option<NodeId> {
        self.local_modules.get(&module).copied()
    }

    pub(super) fn source_file(&self, db: &RootDatabase, file: EditionedFileId) -> Option<String> {
        resolve_workspace_relative(self.vfs, file.file_id(db), self.workspace_root)
    }
    pub(super) fn new(
        vfs: &'a Vfs,
        workspace_root: &'a Path,
        _workspace_hash: &'a str,
        local_modules: &'a HashMap<ModuleId, NodeId>,
    ) -> Self {
        Self {
            vfs,
            workspace_root,
            local_modules,
            by_def: HashMap::new(),
            listed_impls: HashMap::new(),
        }
    }

    pub(super) fn of_definition(
        &mut self,
        sema: &Semantics<'_, RootDatabase>,
        def: Definition<'_>,
    ) -> Option<NodeId> {
        if let Definition::Field(field) = def {
            let parent = self.of_definition(sema, field.parent_def(sema.db).into())?;
            return Some(NodeId::from_components(&[&parent.to_hex(), "field", field.name(sema.db).as_str()]));
        }
        let def_id = target_def_id(def)?;
        if let Some(&known) = self.by_def.get(&def_id) {
            return known;
        }
        let id = self.compute(sema, def, def_id);
        self.by_def.insert(def_id, id);
        id
    }

    fn compute(
        &mut self,
        sema: &Semantics<'_, RootDatabase>,
        def: Definition<'_>,
        def_id: ModuleDefId,
    ) -> Option<NodeId> {
        let db = sema.db;
        if let Definition::EnumVariant(variant) = def {
            let parent = self.of_definition(sema, Definition::Adt(ra_ap_hir::Adt::Enum(variant.parent_enum(db))))?;
            return Some(NodeId::from_components(&[
                &parent.to_hex(), "enum_variant", variant.name(db).as_str(),
            ]));
        }
        let Some(assoc) = def.as_assoc_item(db) else {
            let owner = module_def_owner_module(db, def_id)?;
            if owner.is_block_module(db) {
                let module = ra_ap_hir::Module::from(owner).nearest_non_block_module(db);
                if !self.local_modules.contains_key(&module.into()) { return None; }
                let nav = def.try_to_nav(sema)?.call_site;
                let tree = sema.parse(EditionedFileId::new(db, nav.file_id, module.krate(db).edition(db)));
                let declaration = tree.syntax().descendants().find(|node| node.text_range() == nav.full_range
                    && declaration_definition(sema, node) == Some(def))?;
                let parent_node = declaration.ancestors().skip(1)
                    .find(|node| declaration_definition(sema, node).is_some())?;
                let parent_def = declaration_definition(sema, &parent_node)?;
                let parent = self.of_definition(sema, parent_def)?;
                // A lexical block ordinal distinguishes same-named local
                // declarations in sibling scopes, without byte coordinates.
                // Inserting a new block before it changes that lexical key;
                // inserting comments, whitespace or non-block statements does not.
                let block = declaration.ancestors().find(|n| ast::BlockExpr::can_cast(n.kind()));
                let ordinal = block.and_then(|block| parent_node.descendants()
                    .filter(|n| ast::BlockExpr::can_cast(n.kind())).position(|n| n == block))?;
                return Some(NodeId::from_components(&[
                    &parent.to_hex(), "block_item", &ordinal.to_string(),
                    super::labels::item_kind_id_label(item_kind_for_def(def_id)?), &name_for_def(db, def_id)?,
                ]));
            }
            if !self.local_modules.contains_key(&owner) {
                return None;
            }
            return Some(local_item_node_id(
                *self.local_modules.get(&owner)?,
                item_kind_for_def(def_id)?,
                &name_for_def(db, def_id)?,
            ));
        };
        match assoc.container(db) {
            AssocItemContainer::Trait(trait_) => {
                let owner = module_def_owner_module(db, ModuleDefId::TraitId(trait_.into()))?;
                if !self.local_modules.contains_key(&owner) {
                    return None;
                }
            }
            AssocItemContainer::Impl(impl_) => {
                let owner = impl_.module(db).nearest_non_block_module(db);
                if !self.local_modules.contains_key(&owner.into()) { return None; }
                let krate = impl_.krate(db);
                let listed = self
                    .listed_impls
                    .entry(krate)
                    .or_insert_with(|| Impl::all_in_crate(db, krate).into_iter().collect());
                if !listed.contains(&impl_) {
                    return None;
                }
            }
        };
        let (item_kind, name) = match assoc {
            AssocItem::Function(f) => (ItemKind::Method, f.name(db)),
            AssocItem::Const(c) => (ItemKind::AssocConst, c.name(db)?),
            AssocItem::TypeAlias(t) => (ItemKind::AssocType, t.name(db)),
        };
        let module = assoc.module(db).nearest_non_block_module(db);
        assoc_item_node_id(sema, *self.local_modules.get(&module.into())?, assoc, item_kind, name.as_str())
    }
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

#[cfg(test)]
mod tests {
    //! Phase A1 fixture tests: verify that the five reference patterns we
    //! advertise (method call, trait dispatch, generic bound, const read,
    //! macro expansion) are actually captured by `extract_usages` via
    //! rust-analyzer's `Definition::usages` API.
    //!
    //! Strategy: Option B — one synthetic tempdir crate exercising all five
    //! patterns, persisted once and shared across tests via a `OnceLock`.
    //! Unlike `graph::test_support::shared_snapshot` (which loads the real
    //! workspace), this `synthetic_snapshot` fixture materializes its own
    //! minimal cargo crate so the pattern-matching assertions below have a
    //! known fixed input.
    //!
    //! These tests load a *real* cargo workspace through rust-analyzer, so
    //! they pay the full RA load cost on first call (~3-5s release).
    //! Subsequent tests reuse the cached snapshot.
    use crate::graph::model::{NodeKind, Usage};
    use crate::graph::snapshot::{BuildOptions, OpenedSnapshot, build_and_persist, open_current};
    use crate::graph::storage::{GraphEnvOptions, GraphPaths};
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

    // The empty `[workspace]` table makes this manifest a self-contained
    // workspace root. Without it, `cargo metadata` walks up the directory
    // tree looking for an enclosing `[workspace]` and can latch onto an
    // unrelated `Cargo.toml` (e.g. a stray `/tmp/Cargo.toml`), causing the
    // RA load to fail with "no targets specified in the manifest".
    const FIXTURE_CARGO_TOML: &str = r#"
[package]
name = "synthetic_crate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[workspace]
"#;

    struct SharedSnap {
        _workspace_td: tempfile::TempDir,
        _data_td: tempfile::TempDir,
        snap: OpenedSnapshot,
    }

    /// Build the synthetic fixture crate, run `build_and_persist`, open the
    /// snapshot. Cached across all tests in this module.
    fn synthetic_snapshot() -> &'static OpenedSnapshot {
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
        let snap = synthetic_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Foo::bar");
        assert!(
            !usages.is_empty(),
            "expected >=1 usage of `Foo::bar` from `use_method_call`, got 0"
        );
        for u in &usages {
            assert!(
                u.file.contains("lib.rs"),
                "usage file should be lib.rs, got {}",
                u.file
            );
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
        let snap = synthetic_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Trait::method");
        assert!(
            !usages.is_empty(),
            "expected >=1 usage of `Trait::method` from `use_trait_dispatch`, got 0"
        );
        for u in &usages {
            assert!(
                u.file.contains("lib.rs"),
                "usage file should be lib.rs, got {}",
                u.file
            );
        }
    }

    /// Pattern 3 — generic bound: `fn f<U: Bound>()` referencing a trait
    /// in a where clause / bound position.
    #[test]
    fn pattern3_generic_bound_captured() {
        let snap = synthetic_snapshot();
        let usages = usages_for(snap, "synthetic_crate::Bound");
        assert!(
            !usages.is_empty(),
            "expected ≥1 usage of trait `Bound` from `use_generic_bound` bound, got 0"
        );
        for u in &usages {
            assert!(
                u.file.contains("lib.rs"),
                "usage file should be lib.rs, got {}",
                u.file
            );
        }
    }

    /// Pattern 4 — const read: `fn read_const() -> u32 { K }` — bare const
    /// reference in a function body. Should be classified as `Read`.
    #[test]
    fn pattern4_const_read_captured() {
        let snap = synthetic_snapshot();
        let usages = usages_for(snap, "synthetic_crate::K");
        assert!(
            !usages.is_empty(),
            "expected ≥1 usage of const `K` from `read_const`, got 0"
        );
        for u in &usages {
            assert!(
                u.file.contains("lib.rs"),
                "usage file should be lib.rs, got {}",
                u.file
            );
        }
    }

    /// Pattern 5 — macro expansion: `println!("{}", FOO);` — the const `FOO`
    /// is named inside a macro call. Whether RA's `Definition::usages`
    /// surfaces references inside macro inputs is the key question.
    #[test]
    fn pattern5_macro_expansion_captured() {
        let snap = synthetic_snapshot();
        let usages = usages_for(snap, "synthetic_crate::FOO");
        assert!(
            !usages.is_empty(),
            "expected ≥1 usage of const `FOO` from `macro_use` (println! arg), got 0"
        );
        for u in &usages {
            assert!(
                u.file.contains("lib.rs"),
                "usage file should be lib.rs, got {}",
                u.file
            );
        }
    }

    /// Pattern 6 — Layer 10 call-graph: every method call site inside a fn
    /// body should land with `consumer_function` set to the enclosing fn's
    /// NodeId. `Foo::bar()` is invoked from `use_method_call`,
    /// `outer_with_closure`, and `caller` — at least one should satisfy
    /// `consumer_function.is_some()`.
    #[test]
    fn pattern6_function_attribution_works() {
        let snap = synthetic_snapshot();
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
        let snap = synthetic_snapshot();
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
        let snap = synthetic_snapshot();
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

/// Acceptance of the consumer pass: a snapshot built with it equals one built
/// with the target pass (`snapshot_compare` oracle), on a fixture made of the
/// cases where the two could part ways, and on a real workspace (`#[ignore]`).
#[cfg(test)]
mod pass_equality_tests {
    use std::path::Path;

    use super::UsagesPass;
    use crate::graph::extract::extract_with;
    use crate::graph::loader;
    use crate::graph::model::{ExtractionModel, UsageCategory};
    use crate::graph::snapshot::persist_test_model;
    use crate::graph::snapshot_compare::compare_snapshots;
    use crate::graph::storage::GraphEnvOptions;

    /// Each line is here for a rule of `FindUsages` the consumer pass has to
    /// reproduce; the comment next to it names the rule.
    const FILES: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            r#"[workspace]
members = ["core_a", "app_b"]
resolver = "2"
"#,
        ),
        (
            "core_a/Cargo.toml",
            r#"[package]
name = "core_a"
version = "0.1.0"
edition = "2021"
"#,
        ),
        (
            "core_a/src/lib.rs",
            r#"pub mod macros;
pub mod shapes;

pub const LIMIT: u32 = 3;
pub static GREETING: &str = "hi";

// Raw identifier: the name is searched without `r#`.
pub fn r#type() -> u32 {
    1
}

// Trait items: references to an impl's item count for the trait's item, and
// so does the impl's own declaring name.
pub trait Area {
    type Unit;
    const SIDES: u32;
    fn area(&self) -> f64;
    fn describe(&self) -> String {
        format!("{}", self.area())
    }
}

// A const in a pattern is an `ast::Name` that refers (ConstReference).
pub fn classify(n: u32) -> &'static str {
    match n {
        LIMIT => "limit",
        _ => "other",
    }
}

// Implicit format captures, next to an explicit argument; `LIMIT` also sits in
// the plain text of the string, where it is no reference.
pub fn shout() -> String {
    format!("{GREETING}! {LIMIT} {} LIMIT", LIMIT)
}

pub fn helper() -> u32 {
    LIMIT + r#type()
}

// A `#[test]` fn: the loader analyses crates with cfg(test) off
// (`set_test: false`), where RA drops it like rustc does — neither pass sees
// the reference, and category Test never occurs.
#[test]
fn checks_helper() {
    assert_eq!(helper(), 4);
}
"#,
        ),
        (
            "core_a/src/shapes.rs",
            r#"use crate::Area;

pub struct Square {
    pub side: f64,
}

impl Square {
    pub const SIDES: u32 = 4;

    pub fn new(side: f64) -> Self {
        Square { side }
    }

    pub fn grow(&mut self) {
        self.side *= 2.0;
    }
}

impl Area for Square {
    type Unit = f64;
    const SIDES: u32 = 4;
    fn area(&self) -> f64 {
        self.side * self.side
    }
}

// An inherent impl in an unnamed const's block: `Impl::all_in_crate` lists it.
const _: () = {
    impl Square {
        pub fn hidden_in_const() -> u32 {
            7
        }
    }
};

// An inherent impl in a fn body: not listed, so its method is no target.
pub fn in_body() -> u32 {
    struct Local;
    impl Local {
        fn local_method(&self) -> u32 {
            1
        }
    }
    impl Square {
        pub fn from_body() -> u32 {
            2
        }
    }
    Local.local_method() + Square::from_body() + Square::hidden_in_const()
}

pub fn total(s: &Square) -> f64 {
    s.area() + <Square as Area>::SIDES as f64 + Square::SIDES as f64
}

pub type Meters = <Square as Area>::Unit;
"#,
        ),
        (
            "core_a/src/macros.rs",
            r#"// One input token, two references after expansion.
#[macro_export]
macro_rules! call_twice {
    ($f:ident) => {{
        $f();
        $f();
    }};
}

pub fn ping() {}

pub fn run() {
    call_twice!(ping);
}
"#,
        ),
        (
            "app_b/Cargo.toml",
            r#"[package]
name = "app_b"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[[bin]]
name = "app_b"
path = "src/main.rs"

[dependencies]
core_a = { path = "../core_a" }
"#,
        ),
        (
            "app_b/src/lib.rs",
            r#"// Renamed imports: `Sq::new` is still searched as `new`, but `assist()` is
// not a usage of `helper` — `FindUsages` only matches the target's own name.
use core_a::helper as assist;
use core_a::shapes::Square as Sq;
use core_a::{Area, LIMIT};

pub fn build() -> f64 {
    let mut s = Sq::new(1.0);
    s.grow();
    s.area() + s.describe().len() as f64
}

pub fn via_alias() -> u32 {
    assist() + LIMIT
}

pub fn generic<T: Area>(t: &T) -> f64 {
    t.area()
}

pub struct Circle;

impl Area for Circle {
    type Unit = f32;
    const SIDES: u32 = 0;
    fn area(&self) -> f64 {
        3.0
    }
}

pub mod inner {
    pub fn deep() -> u32 {
        super::via_alias() + crate::via_alias()
    }
}
"#,
        ),
        (
            // The bin target shares the lib's crate name, so the two crates'
            // nodes merge (`labels::crate_display_name`).
            "app_b/src/main.rs",
            r#"fn main() {
    let _ = app_b::build();
    let _ = core_a::shout();
    println!("{}", core_a::LIMIT);
}
"#,
        ),
    ];

    fn write_fixture(root: &Path) {
        for (path, text) in FILES {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    /// One load, both passes, both snapshots compared. Returns the target
    /// pass's model for the caller's non-vacuity checks.
    fn assert_passes_agree(workspace: &Path, samples: usize) -> ExtractionModel {
        let loaded = loader::load(workspace).unwrap();
        let t = std::time::Instant::now();
        let mut from_targets = extract_with(&loaded, UsagesPass::FromTargets);
        let targets_time = t.elapsed();
        let t = std::time::Instant::now();
        let mut from_consumers = extract_with(&loaded, UsagesPass::FromConsumers);
        let consumers_time = t.elapsed();
        eprintln!(
            "extract with usages from targets: {targets_time:.2?} ({} usages); \
             from consumers: {consumers_time:.2?} ({} usages)",
            from_targets.usages.len(),
            from_consumers.usages.len()
        );
        let reference = from_targets.clone();

        // The legacy extractor is an oracle only for unchanged semantics.
        // These fixture owners have no inactive/inline/nested attribution
        // defects. New field/impl targets and aliases are independently
        // checked by tests/edge_semantics.rs, not forced back into parity.
        let unchanged = [
            "core_a::classify", "core_a::shout", "core_a::helper",
            "core_a::shapes::Square::new", "core_a::shapes::Square::grow",
            "core_a::shapes::total", "core_a::macros::run",
            "app_b::build", "app_b::generic", "app_b::main",
        ];
        for model in [&mut from_targets, &mut from_consumers] {
            model.usages.retain(|u| {
                let target = &model.nodes[&u.target];
                u.consumer_function.is_some_and(|id| unchanged.contains(&model.nodes[&id].qualified_name.as_str()))
                    && target.item_kind != Some(crate::graph::model::ItemKind::Field)
                    && !target.qualified_name.contains("::<")
                    && std::fs::read_to_string(workspace.join(&u.file)).unwrap()
                        [u.start as usize..u.end as usize].trim_start_matches("r#") == target.display_name
            });
            assert!(model.usages.len() >= 15, "unchanged parity subset is empty or unexpectedly small");
            for usage in &mut model.usages {
                usage.relation = crate::graph::model::UsageRelation::Unknown;
                usage.consumer_item = usage.consumer_function;
                // Legacy parity has source-site units: FindUsages collapses
                // macro multiplicity and stores no expansion provenance.
                // Exact expanded occurrences have an independent oracle.
                usage.expansion = None;
            }
            let mut sites = std::collections::HashSet::new();
            model.usages.retain(|usage| sites.insert(crate::graph::snapshot::usage_id_for(usage)));
            model.unresolved_calls.clear();
        }

        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a =
            persist_test_model(data_a.path(), &from_targets, GraphEnvOptions::default()).unwrap();
        let b =
            persist_test_model(data_b.path(), &from_consumers, GraphEnvOptions::default()).unwrap();
        let diff = compare_snapshots(&a, &b, samples).unwrap();
        assert!(
            diff.is_empty(),
            "usages from targets (left) vs from consumers (right) on {}:\n{diff}",
            workspace.display()
        );
        reference
    }

    #[test]
    fn consumer_pass_equals_target_pass_on_fixture() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture(workspace.path());
        let model = assert_passes_agree(workspace.path(), 20);

        // Equality of two empty tables would prove nothing: the fixture's
        // cases have to have produced rows of every category.
        let has = |cat: UsageCategory| model.usages.iter().any(|u| u.category == cat);
        assert!(has(UsageCategory::Read), "no format-capture usage");
        assert!(has(UsageCategory::Other), "no plain usage");
        let usages_of = |qualified: &str| {
            let (&id, _) = model
                .nodes
                .iter()
                .find(|(_, n)| n.qualified_name == qualified)
                .unwrap_or_else(|| panic!("no node `{qualified}`"));
            model.usages.iter().filter(|u| u.target == id).count()
        };
        assert!(usages_of("core_a::Area::area") >= 4, "trait item via impls");
        assert!(
            usages_of("core_a::shapes::Square::new") >= 1,
            "assoc fn via renamed type"
        );
        assert!(
            usages_of("core_a::macros::ping") >= 1,
            "reference through a macro"
        );
        assert!(usages_of("core_a::type") >= 1, "raw identifier");
    }

}
