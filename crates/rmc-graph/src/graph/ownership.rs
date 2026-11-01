//! Which crate owns each row of a snapshot — so that one crate's rows can be
//! deleted and written again without touching anyone else's.
//!
//! # The rule
//!
//! Every row a build writes has exactly one owning crate (the crate's NodeId):
//!
//! - a node, its `signatures_by_target` / `static_metadata_by_target` record
//!   and its `embeddings_by_target` cache entry — the crate the item is
//!   declared in (`Node.crate_id`; a crate node owns itself);
//! - a `children_by_parent` edge — the crate of the child (for the
//!   workspace → crate edge, that crate);
//! - a binding and its two index rows — the crate of `from_module`;
//! - a usage and its index rows — the crate of the CONSUMER
//!   (`consumer_module`), not of the target: editing a consumer must not
//!   touch rows filed under the producer.
//!
//! Two kinds of node are shared instead:
//!
//! - the workspace node is the same for every build of a workspace; every
//!   write puts it and no crate deletion removes it;
//! - an ExternalSymbol stub is named by bindings of possibly many crates.
//!   `stub_owners` holds one `stub → crate` row per such crate (that row is
//!   owned by its crate like any other), and the stub node is deleted when
//!   its last `stub_owners` row is. The signature / static-metadata passes
//!   walk every def a binding reached, external ones included, so a stub can
//!   carry those records too; they share the stub's lifetime.
//!
//! # The record
//!
//! `owned_rows_by_crate` (DUP_SORT) holds, under each crate, one [`OwnedRow`]
//! per row it owns: the table, the key and — for a DUP_SORT table — the value,
//! which is what LMDB needs to delete exactly that duplicate. It is the
//! complete list of what [`delete_crate_rows`] removes, with one exception:
//! `embeddings_by_target` entries are written lazily after the build, by
//! `semantic_overlaps`, so they are not listed and go with their node's
//! `nodes_by_id` row instead.

use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};
use heed::RwTxn;

use super::ids::NodeId;
use super::model::{ExtractionModel, NodeKind};
use super::snapshot::{binding_id_for, usage_id_for};
use super::storage::{DbTag, GraphDatabases};

const ID_LEN: usize = 32;

/// One row a crate owns: enough to delete exactly that row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnedRow {
    pub tag: DbTag,
    pub key: [u8; ID_LEN],
    /// The value, for a DUP_SORT table (`tag.is_dup_sort()`); `None` for a
    /// table with one value per key, where the key alone names the row.
    pub dup: Option<[u8; ID_LEN]>,
}

impl OwnedRow {
    /// `tag code (1 byte) ‖ key (32) ‖ dup value (32, DUP_SORT tables only)`.
    ///
    /// The table code comes first, so under one crate LMDB keeps the rows
    /// grouped by table.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 2 * ID_LEN);
        out.push(self.tag.code());
        out.extend_from_slice(&self.key);
        if let Some(dup) = &self.dup {
            out.extend_from_slice(dup);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (&code, rest) = bytes.split_first().context("empty owned row")?;
        let tag = DbTag::from_code(code).with_context(|| format!("unknown table code {code}"))?;
        ensure!(
            tag != DbTag::OwnedRowsByCrate,
            "an owned row cannot point into `owned_rows_by_crate` itself"
        );
        let expected = if tag.is_dup_sort() {
            2 * ID_LEN
        } else {
            ID_LEN
        };
        ensure!(
            rest.len() == expected,
            "owned `{tag}` row has {} bytes after the code, expected {expected}",
            rest.len()
        );
        let key: [u8; ID_LEN] = rest[..ID_LEN].try_into().unwrap();
        let dup = tag
            .is_dup_sort()
            .then(|| rest[ID_LEN..].try_into().unwrap());
        Ok(Self { tag, key, dup })
    }
}

/// How many model entries a write put into the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct WrittenCounts {
    /// Distinct nodes, the workspace node and the stubs included.
    pub nodes: u64,
    /// Entries of `model.bindings` (a repeated binding id counts each time,
    /// as the manifest always has).
    pub bindings: u64,
    /// Entries of `model.usages`, counted the same way.
    pub usages: u64,
}

/// Write the rows of `model` owned by the crates `keep` accepts, each with its
/// ownership record, plus the workspace node and the stubs those crates name.
///
/// A full build passes `|_| true`; rewriting one crate after
/// [`delete_crate_rows`] passes `|c| c == that_crate`. Fails if a node, edge,
/// binding or usage has no owner — a model inconsistency that would otherwise
/// leave a row no crate deletion can reach.
pub(crate) fn write_owned_rows(
    wtxn: &mut RwTxn<'_>,
    dbs: &GraphDatabases,
    model: &ExtractionModel,
    keep: &dyn Fn(NodeId) -> bool,
) -> Result<WrittenCounts> {
    let mut counts = WrittenCounts::default();
    let own = |wtxn: &mut RwTxn<'_>, owner: NodeId, row: OwnedRow| -> Result<()> {
        dbs.owned_rows_by_crate
            .put(wtxn, owner.as_bytes(), &row.encode())
            .context("put owned row")
    };

    // 1. Nodes. Stubs are written below, with the bindings that name them.
    for node in model.nodes.values() {
        match node.kind {
            NodeKind::Workspace => {}
            NodeKind::ExternalSymbol => continue,
            NodeKind::Crate | NodeKind::Module | NodeKind::Item => {
                let owner = node.crate_id.with_context(|| {
                    format!(
                        "{:?} node `{}` has no crate",
                        node.kind, node.qualified_name
                    )
                })?;
                if !keep(owner) {
                    continue;
                }
                own(
                    wtxn,
                    owner,
                    OwnedRow {
                        tag: DbTag::NodesById,
                        key: node.id.0,
                        dup: None,
                    },
                )?;
            }
        }
        dbs.nodes_by_id
            .put(wtxn, node.id.as_bytes(), node)
            .context("put node")?;
        counts.nodes += 1;
    }

    // 2. Bindings + per-from-module / per-target indexes, and the stubs they
    // name.
    let mut stubs_written: HashSet<NodeId> = HashSet::new();
    for binding in &model.bindings {
        let owner = owner_of(model, binding.from_module)?;
        if !keep(owner) {
            continue;
        }
        let bid = binding_id_for(binding);
        dbs.bindings_by_id
            .put(wtxn, bid.as_bytes(), binding)
            .context("put binding")?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::BindingsById,
                key: bid.0,
                dup: None,
            },
        )?;
        dbs.bindings_by_from_module
            .put(wtxn, binding.from_module.as_bytes(), bid.as_bytes())?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::BindingsByFromModule,
                key: binding.from_module.0,
                dup: Some(bid.0),
            },
        )?;
        dbs.bindings_by_target
            .put(wtxn, binding.target.as_bytes(), bid.as_bytes())?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::BindingsByTarget,
                key: binding.target.0,
                dup: Some(bid.0),
            },
        )?;
        counts.bindings += 1;

        let Some(stub) = model
            .nodes
            .get(&binding.target)
            .filter(|n| n.kind == NodeKind::ExternalSymbol)
        else {
            continue;
        };
        if stubs_written.insert(stub.id) {
            dbs.nodes_by_id
                .put(wtxn, stub.id.as_bytes(), stub)
                .context("put stub node")?;
            counts.nodes += 1;
        }
        dbs.stub_owners
            .put(wtxn, stub.id.as_bytes(), owner.as_bytes())?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::StubOwners,
                key: stub.id.0,
                dup: Some(owner.0),
            },
        )?;
    }

    // 3. Contains hierarchy, owned by the child's crate.
    for &(parent, child) in &model.contains {
        let owner = owner_of(model, child)?;
        if !keep(owner) {
            continue;
        }
        dbs.children_by_parent
            .put(wtxn, parent.as_bytes(), child.as_bytes())?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::ChildrenByParent,
                key: parent.0,
                dup: Some(child.0),
            },
        )?;
    }

    // 4. Usages + per-target / per-consumer indexes, owned by the consumer.
    for usage in &model.usages {
        let owner = owner_of(model, usage.consumer_module)?;
        if !keep(owner) {
            continue;
        }
        let uid = usage_id_for(usage);
        dbs.usages_by_id
            .put(wtxn, uid.as_bytes(), usage)
            .context("put usage")?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::UsagesById,
                key: uid.0,
                dup: None,
            },
        )?;
        dbs.usages_by_target
            .put(wtxn, usage.target.as_bytes(), uid.as_bytes())?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::UsagesByTarget,
                key: usage.target.0,
                dup: Some(uid.0),
            },
        )?;
        dbs.usages_by_consumer
            .put(wtxn, usage.consumer_module.as_bytes(), uid.as_bytes())?;
        own(
            wtxn,
            owner,
            OwnedRow {
                tag: DbTag::UsagesByConsumer,
                key: usage.consumer_module.0,
                dup: Some(uid.0),
            },
        )?;
        if let Some(consumer_fn) = usage.consumer_function {
            dbs.usages_by_consumer_function
                .put(wtxn, consumer_fn.as_bytes(), uid.as_bytes())?;
            own(
                wtxn,
                owner,
                OwnedRow {
                    tag: DbTag::UsagesByConsumerFunction,
                    key: consumer_fn.0,
                    dup: Some(uid.0),
                },
            )?;
        }
        counts.usages += 1;
        if let Some(item) = usage.consumer_item {
            dbs.usages_by_consumer_item.put(wtxn, item.as_bytes(), uid.as_bytes())?;
            own(wtxn, owner, OwnedRow { tag: DbTag::UsagesByConsumerItem, key: item.0, dup: Some(uid.0) })?;
        }
    }

    let mut unresolved = std::collections::BTreeMap::<_, Vec<_>>::new();
    for call in &model.unresolved_calls {
        let owner = owner_of(model, call.consumer_module)?;
        if !keep(owner) { continue; }
        let item = call.consumer_item.unwrap_or(call.consumer_module);
        unresolved.entry(item).or_default().push(call.clone());
    }
    for (item, mut calls) in unresolved {
        calls.sort_by(|a, b| (&a.file, a.start, a.end, &a.expression).cmp(&(&b.file, b.start, b.end, &b.expression)));
        calls.dedup();
        let owner = owner_of(model, calls[0].consumer_module)?;
        dbs.unresolved_calls_by_owner.put(wtxn, item.as_bytes(), &calls)?;
        own(wtxn, owner, OwnedRow { tag: DbTag::UnresolvedCallsByOwner, key: item.0, dup: None })?;
    }

    // 5. Signatures (v9) and static metadata (v10): one record per item,
    // owned with the item's node. The two passes walk every def a binding
    // reached, external ones too, so a stub can have a record: it is shared
    // with the stub — written with it, deleted with it, listed under no crate.
    for (target, sig) in &model.signatures {
        let Some(owner) = record_owner(model, *target, &stubs_written, keep)? else {
            continue;
        };
        dbs.signatures_by_target
            .put(wtxn, target.as_bytes(), sig)
            .context("put signature")?;
        if let Owner::Crate(owner) = owner {
            own(
                wtxn,
                owner,
                OwnedRow {
                    tag: DbTag::SignaturesByTarget,
                    key: target.0,
                    dup: None,
                },
            )?;
        }
    }
    for (target, meta) in &model.statics {
        let Some(owner) = record_owner(model, *target, &stubs_written, keep)? else {
            continue;
        };
        dbs.static_metadata_by_target
            .put(wtxn, target.as_bytes(), meta)
            .context("put static metadata")?;
        if let Owner::Crate(owner) = owner {
            own(
                wtxn,
                owner,
                OwnedRow {
                    tag: DbTag::StaticMetadataByTarget,
                    key: target.0,
                    dup: None,
                },
            )?;
        }
    }

    Ok(counts)
}

/// Who a per-item record (signature, static metadata) belongs to.
enum Owner {
    Crate(NodeId),
    /// The record of an ExternalSymbol stub: it lives as long as the stub.
    Stub,
}

/// The owner of the record keyed by `target`, or `None` when this write
/// leaves the record out — its crate is not kept, or it belongs to a stub
/// none of the kept crates names.
fn record_owner(
    model: &ExtractionModel,
    target: NodeId,
    stubs_written: &HashSet<NodeId>,
    keep: &dyn Fn(NodeId) -> bool,
) -> Result<Option<Owner>> {
    let is_stub = model
        .nodes
        .get(&target)
        .is_some_and(|n| n.kind == NodeKind::ExternalSymbol);
    if is_stub {
        return Ok(stubs_written.contains(&target).then_some(Owner::Stub));
    }
    let owner = owner_of(model, target)?;
    Ok(keep(owner).then_some(Owner::Crate(owner)))
}

/// Delete every row `krate` owns (see the module docs for the rule), the
/// embeddings cached for its nodes, and the stubs no other crate names any
/// more. Returns the number of owned rows deleted.
///
/// Fails if a listed row is not in its table: the ownership record and the
/// tables disagree, and going on would leave the snapshot half-deleted in a
/// way no later check could tell apart from a correct one.
pub(crate) fn delete_crate_rows(
    wtxn: &mut RwTxn<'_>,
    dbs: &GraphDatabases,
    krate: NodeId,
) -> Result<usize> {
    let rows: Vec<OwnedRow> = match dbs
        .owned_rows_by_crate
        .get_duplicates(wtxn, krate.as_bytes())?
    {
        None => Vec::new(),
        Some(iter) => iter
            .map(|entry| OwnedRow::decode(entry?.1))
            .collect::<Result<_>>()?,
    };

    let mut stubs: Vec<[u8; ID_LEN]> = Vec::new();
    for row in &rows {
        let table = row.tag.raw(dbs);
        let deleted = match &row.dup {
            None => table.delete(wtxn, &row.key)?,
            Some(dup) => table.delete_one_duplicate(wtxn, &row.key, dup)?,
        };
        if !deleted {
            bail!(
                "crate {krate:?} owns a `{}` row that is not in the table: {row:?}",
                row.tag
            );
        }
        match row.tag {
            DbTag::NodesById => {
                dbs.embeddings_by_target.delete(wtxn, &row.key)?;
            }
            DbTag::StubOwners => stubs.push(row.key),
            _ => {}
        }
    }
    // A stub with no owner left goes, and its per-item records with it.
    for stub in stubs {
        if dbs.stub_owners.get(wtxn, &stub)?.is_none() {
            dbs.nodes_by_id.delete(wtxn, &stub)?;
            dbs.signatures_by_target.delete(wtxn, &stub)?;
            dbs.static_metadata_by_target.delete(wtxn, &stub)?;
            dbs.embeddings_by_target.delete(wtxn, &stub)?;
        }
    }
    dbs.owned_rows_by_crate.delete(wtxn, krate.as_bytes())?;
    Ok(rows.len())
}

/// The crate that owns rows filed under `node`: its `crate_id`.
fn owner_of(model: &ExtractionModel, node: NodeId) -> Result<NodeId> {
    let n = model
        .nodes
        .get(&node)
        .with_context(|| format!("row points at {node:?}, which is not a node of the model"))?;
    n.crate_id.with_context(|| {
        format!(
            "{:?} node `{}` owns rows but has no crate",
            n.kind, n.qualified_name
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use super::*;
    use crate::graph::snapshot::{OpenedSnapshot, persist_test_model};
    use crate::graph::snapshot_compare::{SnapshotRow, compare_snapshots, dump_snapshot};
    use crate::graph::storage::GraphEnvOptions;
    use crate::graph::test_support::write_two_crate_fixture;
    use crate::graph::{extract, loader};

    fn model_of(workspace: &Path) -> ExtractionModel {
        extract::extract(&loader::load(workspace).unwrap())
    }

    fn crate_id(model: &ExtractionModel, name: &str) -> NodeId {
        model
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Crate && n.qualified_name == name)
            .unwrap_or_else(|| panic!("no crate `{name}` in the model"))
            .id
    }

    fn persist(model: &ExtractionModel) -> (tempfile::TempDir, OpenedSnapshot) {
        let data = tempfile::tempdir().unwrap();
        let snap = persist_test_model(data.path(), model, GraphEnvOptions::default()).unwrap();
        (data, snap)
    }

    /// Owners of each `(table, key, dup)` row, read back from the snapshot.
    fn owners_by_row(rows: &BTreeSet<SnapshotRow>) -> BTreeMap<OwnedRow, Vec<NodeId>> {
        let mut out: BTreeMap<OwnedRow, Vec<NodeId>> = BTreeMap::new();
        for (tag, key, value) in rows {
            if *tag == DbTag::OwnedRowsByCrate {
                let owner = NodeId(key.as_slice().try_into().unwrap());
                out.entry(OwnedRow::decode(value).unwrap())
                    .or_default()
                    .push(owner);
            }
        }
        out
    }

    /// The ownership check on a whole snapshot: every row but the shared
    /// nodes has exactly one owner and every ownership record names a row
    /// that exists; every stub has an owner and nothing else is unowned.
    fn assert_partitioned(snap: &OpenedSnapshot) {
        let rows = dump_snapshot(snap).unwrap();
        let owners = owners_by_row(&rows);
        let rtxn = snap.read_txn().unwrap();
        let mut seen: BTreeSet<OwnedRow> = BTreeSet::new();
        for (tag, key, value) in &rows {
            if *tag == DbTag::OwnedRowsByCrate {
                continue;
            }
            let row = OwnedRow {
                tag: *tag,
                key: key.as_slice().try_into().unwrap(),
                dup: tag
                    .is_dup_sort()
                    .then(|| value.as_slice().try_into().unwrap()),
            };
            seen.insert(row);
            let n = owners.get(&row).map_or(0, Vec::len);
            // Rows keyed by a node: the node, its signature, its static
            // metadata. Those of the shared nodes are owned by no crate.
            if matches!(
                tag,
                DbTag::NodesById | DbTag::SignaturesByTarget | DbTag::StaticMetadataByTarget
            ) {
                let node = snap
                    .dbs
                    .nodes_by_id
                    .get(&rtxn, key)
                    .unwrap()
                    .unwrap_or_else(|| panic!("`{tag}` row {row:?} names no node"));
                match node.kind {
                    NodeKind::Workspace => {
                        assert_eq!(n, 0, "workspace `{tag}` row is shared, not owned");
                        continue;
                    }
                    NodeKind::ExternalSymbol => {
                        assert_eq!(
                            n, 0,
                            "stub `{}` `{tag}` row goes with the stub, not a crate",
                            node.qualified_name
                        );
                        assert!(
                            snap.dbs.stub_owners.get(&rtxn, key).unwrap().is_some(),
                            "stub `{}` has no owner",
                            node.qualified_name
                        );
                        continue;
                    }
                    _ => {}
                }
            }
            assert_eq!(n, 1, "`{tag}` row {row:?} has {n} owners, expected one");
        }
        for row in owners.keys() {
            assert!(seen.contains(row), "owned row {row:?} is not in its table");
        }
    }

    /// The fixture's stubs: those with two owning crates and those with one.
    fn stub_owner_counts(snap: &OpenedSnapshot) -> BTreeMap<String, usize> {
        let rtxn = snap.read_txn().unwrap();
        let mut out = BTreeMap::new();
        for entry in snap.dbs.stub_owners.iter(&rtxn).unwrap() {
            let (stub, _) = entry.unwrap();
            let node = snap.dbs.nodes_by_id.get(&rtxn, stub).unwrap().unwrap();
            *out.entry(node.qualified_name).or_insert(0) += 1;
        }
        out
    }

    #[test]
    fn owned_row_encoding_round_trips_and_rejects_bad_shapes() {
        let key = [7u8; ID_LEN];
        for row in [
            OwnedRow {
                tag: DbTag::NodesById,
                key,
                dup: None,
            },
            OwnedRow {
                tag: DbTag::UsagesByConsumerFunction,
                key,
                dup: Some([9u8; ID_LEN]),
            },
        ] {
            assert_eq!(OwnedRow::decode(&row.encode()).unwrap(), row);
        }
        // A DUP_SORT table without its value, a plain table with one, an
        // unknown code, and a pointer into the ownership table itself.
        let mut short = vec![DbTag::UsagesByTarget.code()];
        short.extend_from_slice(&key);
        assert!(OwnedRow::decode(&short).is_err());
        let mut long = vec![DbTag::NodesById.code()];
        long.extend_from_slice(&key);
        long.extend_from_slice(&key);
        assert!(OwnedRow::decode(&long).is_err());
        assert!(OwnedRow::decode(&[200u8; 1 + ID_LEN]).is_err());
        let mut own = vec![DbTag::OwnedRowsByCrate.code()];
        own.extend_from_slice(&key);
        own.extend_from_slice(&key);
        assert!(OwnedRow::decode(&own).is_err());
        // Codes are on disk: each tag's code maps back to it.
        for tag in DbTag::ALL {
            assert_eq!(DbTag::from_code(tag.code()), Some(tag));
        }
    }

    #[test]
    fn every_row_of_a_build_has_exactly_one_owner() {
        let workspace = tempfile::tempdir().unwrap();
        write_two_crate_fixture(workspace.path());
        let (_data, snap) = persist(&model_of(workspace.path()));
        assert_partitioned(&snap);

        // Shared and single-owner stubs both exist, or the stub half of the
        // rule is untested.
        let stubs = stub_owner_counts(&snap);
        assert!(
            stubs.values().any(|&n| n == 2),
            "no stub named by both crates: {stubs:?}"
        );
        assert!(
            stubs.values().any(|&n| n == 1),
            "no stub named by one crate: {stubs:?}"
        );
        // …and a stub with a signature, the record that rides on a stub.
        let rtxn = snap.read_txn().unwrap();
        let stub_with_signature = snap.dbs.signatures_by_target.iter(&rtxn).unwrap().any(|e| {
            let (key, _) = e.unwrap();
            let node = snap.dbs.nodes_by_id.get(&rtxn, key).unwrap().unwrap();
            node.kind == NodeKind::ExternalSymbol
        });
        assert!(
            stub_with_signature,
            "no stub carries a signature: {stubs:?}"
        );
    }

    /// Deleting `beta` leaves exactly the snapshot of the same tree with
    /// `beta` taken out of the workspace — including `beta`'s usages of
    /// `alpha` items and the stubs only `beta` named, while the stubs both
    /// crates name stay. Writing `beta` back gives the full snapshot again.
    #[test]
    fn deleting_a_crate_leaves_the_build_without_it_and_rewriting_restores_it() {
        let workspace = tempfile::tempdir().unwrap();
        write_two_crate_fixture(workspace.path());
        let full_model = model_of(workspace.path());
        let beta = crate_id(&full_model, "beta");
        let (_data_full, full) = persist(&full_model);
        let (_data_work, work) = persist(&full_model);

        let mut wtxn = work.write_txn().unwrap();
        let deleted = delete_crate_rows(&mut wtxn, &work.dbs, beta).unwrap();
        wtxn.commit().unwrap();
        assert!(deleted > 0);

        // The same tree, same path (so the same ids), without `beta`.
        std::fs::remove_dir_all(workspace.path().join("beta")).unwrap();
        std::fs::write(
            workspace.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"alpha\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        let alpha_model = model_of(workspace.path());
        let (_data_alpha, alpha_only) = persist(&alpha_model);

        let diff = compare_snapshots(&work, &alpha_only, 10).unwrap();
        assert!(
            diff.is_empty(),
            "full build minus `beta` ≠ build without `beta`:\n{diff}"
        );
        assert!(
            stub_owner_counts(&work).values().all(|&n| n == 1),
            "a stub `beta` named still lists two owners"
        );

        let mut wtxn = work.write_txn().unwrap();
        write_owned_rows(&mut wtxn, &work.dbs, &full_model, &|c| c == beta).unwrap();
        wtxn.commit().unwrap();
        let diff = compare_snapshots(&work, &full, 10).unwrap();
        assert!(diff.is_empty(), "`beta` written back ≠ full build:\n{diff}");
    }

    /// The embeddings cached for a crate's items go with the crate; those of
    /// other crates stay.
    #[test]
    fn deleting_a_crate_drops_its_cached_embeddings_only() {
        let workspace = tempfile::tempdir().unwrap();
        write_two_crate_fixture(workspace.path());
        let model = model_of(workspace.path());
        let beta = crate_id(&model, "beta");
        let item_of = |krate: NodeId| {
            model
                .nodes
                .values()
                .find(|n| n.kind == NodeKind::Item && n.crate_id == Some(krate))
                .unwrap()
                .id
        };
        let (beta_item, alpha_item) = (item_of(beta), item_of(crate_id(&model, "alpha")));
        let (_data, snap) = persist(&model);

        let record = crate::graph::model::EmbeddingRecord {
            content_hash: [0; 16],
            vector: vec![1.0],
            embedder_version: "test".into(),
            generated_at_unix: 0,
        };
        let mut wtxn = snap.write_txn().unwrap();
        for id in [beta_item, alpha_item] {
            snap.dbs
                .embeddings_by_target
                .put(&mut wtxn, id.as_bytes(), &record)
                .unwrap();
        }
        delete_crate_rows(&mut wtxn, &snap.dbs, beta).unwrap();
        wtxn.commit().unwrap();

        let rtxn = snap.read_txn().unwrap();
        assert!(
            snap.dbs
                .embeddings_by_target
                .get(&rtxn, beta_item.as_bytes())
                .unwrap()
                .is_none()
        );
        assert!(
            snap.dbs
                .embeddings_by_target
                .get(&rtxn, alpha_item.as_bytes())
                .unwrap()
                .is_some()
        );
    }

    /// The partition on a real workspace — one full build of
    /// `RMC_SNAPSHOT_COMPARE_WORKSPACE`, then the whole snapshot held in
    /// memory for the check, so never part of the default run.
    #[test]
    #[ignore = "needs RMC_SNAPSHOT_COMPARE_WORKSPACE and a full build of it"]
    fn every_row_of_a_real_workspace_has_exactly_one_owner() {
        let workspace = std::env::var("RMC_SNAPSHOT_COMPARE_WORKSPACE")
            .expect("set RMC_SNAPSHOT_COMPARE_WORKSPACE to a workspace root");
        let (_data, snap) = persist(&model_of(Path::new(&workspace)));
        assert_partitioned(&snap);
    }
}
