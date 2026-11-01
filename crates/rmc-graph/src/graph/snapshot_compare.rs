//! Snapshot ≡ snapshot: the oracle every incremental-build step is judged by.
//!
//! Two snapshots are equal when every sub-DB holds the same `(key, value)`
//! rows, byte for byte — duplicates of a DUP_SORT table included. Two sub-DBs
//! are left out on purpose:
//!
//! - `meta_by_key` carries the fingerprint and the graph id, which differ
//!   between any two builds by construction;
//! - `embeddings_by_target` is a query-time cache filled by
//!   `semantic_overlaps`, not part of what a build produces.
//!
//! Every other sub-DB is compared, and a sub-DB this module does not know is
//! an error rather than a silent skip: a table added to [`GraphDatabases`]
//! without a [`DbTag`] here would otherwise make "no difference" mean "not
//! looked at".
//!
//! # How the comparison runs
//!
//! LMDB keeps each table sorted by key and, under DUP_SORT, each key's values
//! sorted too — with the same byte-wise order as `Vec<u8>`'s `Ord`. So two
//! tables are compared by a streaming merge of their cursors: memory stays flat
//! however large the snapshot is, which is what lets the same oracle judge a
//! full build of a big workspace and not only test fixtures.
//!
//! A difference is reported per table as two counts plus the first few rows
//! on each side, rendered with qualified names instead of hex ids so a
//! mismatch can be traced to its source without a second tool.
//!
//! [`GraphDatabases`]: super::storage::GraphDatabases

use std::collections::BTreeSet;
use std::fmt;

use anyhow::{Context, Result, bail};
use heed::types::{DecodeIgnore, SerdeBincode, Str};
use heed::{BytesDecode, Database, RoTxn, WithoutTls};

use super::ids::{BindingId, NodeId, UsageId};
use super::model::{Binding, FunctionSignature, Node, StaticMetadata, Usage};
use super::ownership::OwnedRow;
use super::snapshot::OpenedSnapshot;
use super::storage::DbTag;

type RoTx<'e> = RoTxn<'e, WithoutTls>;

/// Sub-DBs that exist in every snapshot but are deliberately not compared
/// (see the module docs for why each one). Every other sub-DB is a [`DbTag`].
///
/// Tables are read through [`DbTag::raw`], with key and value types erased:
/// the oracle compares encodings, not decoded values, so a change in how a
/// value serializes is a difference too.
pub const EXCLUDED_TABLES: [&str; 2] = ["meta_by_key", "embeddings_by_target"];

/// One row of one compared table: `(table, key bytes, value bytes)`.
pub type SnapshotRow = (DbTag, Vec<u8>, Vec<u8>);

/// Every compared row of `snap`, as a set.
///
/// Holds the whole snapshot in memory — meant for tests on fixtures and for
/// building mutants. [`compare_snapshots`] does not go through it.
pub fn dump_snapshot(snap: &OpenedSnapshot) -> Result<BTreeSet<SnapshotRow>> {
    let rtxn = snap.read_txn()?;
    check_known_tables(snap, &rtxn)?;
    let mut rows = BTreeSet::new();
    for tag in DbTag::ALL {
        for entry in tag.raw(&snap.dbs).iter(&rtxn)? {
            let (key, value) = entry?;
            rows.insert((tag, key.to_vec(), value.to_vec()));
        }
    }
    Ok(rows)
}

/// What differs in one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub table: DbTag,
    /// Rows present on the left and absent on the right.
    pub only_in_left: usize,
    /// Rows present on the right and absent on the left.
    pub only_in_right: usize,
    /// The first rows of `only_in_left`, rendered (at most the caller's limit).
    pub left_samples: Vec<String>,
    /// The first rows of `only_in_right`, rendered.
    pub right_samples: Vec<String>,
}

/// The difference between two snapshots: only tables that differ are listed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapshotDiff {
    pub tables: Vec<TableDiff>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// The tables that differ, in report order.
    pub fn differing_tables(&self) -> Vec<DbTag> {
        self.tables.iter().map(|t| t.table).collect()
    }
}

impl fmt::Display for SnapshotDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tables.is_empty() {
            return writeln!(f, "snapshots are equal");
        }
        for table in &self.tables {
            writeln!(
                f,
                "{}: {} only in left, {} only in right",
                table.table, table.only_in_left, table.only_in_right
            )?;
            for row in &table.left_samples {
                writeln!(f, "  - {row}")?;
            }
            for row in &table.right_samples {
                writeln!(f, "  + {row}")?;
            }
        }
        Ok(())
    }
}

/// Compare two snapshots table by table; keep at most `samples` rendered rows
/// per side and table.
///
/// "Left" and "right" are the argument order. A changed value under the same
/// non-DUP key shows up as one row on each side.
pub fn compare_snapshots(
    left: &OpenedSnapshot,
    right: &OpenedSnapshot,
    samples: usize,
) -> Result<SnapshotDiff> {
    let ltxn = left.read_txn()?;
    let rtxn = right.read_txn()?;
    check_known_tables(left, &ltxn)?;
    check_known_tables(right, &rtxn)?;
    let names = Names {
        left: (left, &ltxn),
        right: (right, &rtxn),
    };

    let mut diff = SnapshotDiff::default();
    for tag in DbTag::ALL {
        let mut table = TableDiff {
            table: tag,
            only_in_left: 0,
            only_in_right: 0,
            left_samples: Vec::new(),
            right_samples: Vec::new(),
        };
        let mut lit = tag.raw(&left.dbs).iter(&ltxn)?;
        let mut rit = tag.raw(&right.dbs).iter(&rtxn)?;
        let mut l = lit.next().transpose()?;
        let mut r = rit.next().transpose()?;
        loop {
            match (l, r) {
                (None, None) => break,
                (Some(lrow), Some(rrow)) if lrow == rrow => {
                    l = lit.next().transpose()?;
                    r = rit.next().transpose()?;
                }
                (Some(lrow), rrow) if rrow.is_none_or(|rrow| lrow < rrow) => {
                    table.only_in_left += 1;
                    if table.left_samples.len() < samples {
                        table.left_samples.push(names.render(tag, lrow.0, lrow.1));
                    }
                    l = lit.next().transpose()?;
                    r = rrow;
                }
                (lrow, Some(rrow)) => {
                    table.only_in_right += 1;
                    if table.right_samples.len() < samples {
                        table.right_samples.push(names.render(tag, rrow.0, rrow.1));
                    }
                    l = lrow;
                    r = rit.next().transpose()?;
                }
                (Some(_), None) => unreachable!("covered by the `is_none_or` arm"),
            }
        }
        if table.only_in_left + table.only_in_right > 0 {
            diff.tables.push(table);
        }
    }
    Ok(diff)
}

/// Fail on a sub-DB that is neither compared nor deliberately excluded.
///
/// The unnamed main DB of an LMDB environment lists the names of its named
/// sub-DBs as keys, so this is a complete inventory, not a guess.
fn check_known_tables(snap: &OpenedSnapshot, rtxn: &RoTx<'_>) -> Result<()> {
    for name in sub_db_names(snap, rtxn)? {
        let name = name.as_str();
        let known =
            DbTag::ALL.iter().any(|t| t.db_name() == name) || EXCLUDED_TABLES.contains(&name);
        if !known {
            bail!(
                "snapshot {} has sub-DB `{name}`, which the snapshot oracle neither compares nor \
                 excludes; give it a `DbTag` (or list it in `EXCLUDED_TABLES` with the reason)",
                snap.snapshot_dir.display()
            );
        }
    }
    Ok(())
}

/// The names of every named sub-DB in the snapshot's environment.
fn sub_db_names(snap: &OpenedSnapshot, rtxn: &RoTx<'_>) -> Result<Vec<String>> {
    let main: Database<Str, DecodeIgnore> = snap
        .env
        .open_database(rtxn, None)?
        .context("snapshot has no main database")?;
    let mut names = Vec::new();
    for entry in main.iter(rtxn)? {
        let (name, ()) = entry?;
        names.push(name.to_owned());
    }
    Ok(names)
}

/// Renders rows with names, looking ids up in whichever snapshot has them.
///
/// A row that is only on the left usually names things that exist only on
/// the left, so both sides are tried, left first.
struct Names<'a, 'e> {
    left: (&'a OpenedSnapshot, &'a RoTx<'e>),
    right: (&'a OpenedSnapshot, &'a RoTx<'e>),
}

impl Names<'_, '_> {
    fn render(&self, tag: DbTag, key: &[u8], value: &[u8]) -> String {
        match tag {
            DbTag::NodesById => match decode::<Node>(value) {
                Some(node) => format!("{node:?}"),
                None => format!("{} = <undecodable node>", self.node(key)),
            },
            DbTag::BindingsById => match decode::<Binding>(value) {
                Some(binding) => self.binding_text(&binding),
                None => format!("binding {} = <undecodable>", hex(key)),
            },
            DbTag::BindingsByFromModule | DbTag::BindingsByTarget => {
                format!("{} → {}", self.node(key), self.binding(value))
            }
            DbTag::ChildrenByParent => format!("{} ∋ {}", self.node(key), self.node(value)),
            DbTag::UsagesById => match decode::<Usage>(value) {
                Some(usage) => self.usage_text(&usage),
                None => format!("usage {} = <undecodable>", hex(key)),
            },
            DbTag::UsagesByTarget | DbTag::UsagesByConsumer | DbTag::UsagesByConsumerFunction | DbTag::UsagesByConsumerItem => {
                format!("{} → {}", self.node(key), self.usage(value))
            }
            DbTag::SignaturesByTarget => {
                format!(
                    "{}: {:?}",
                    self.node(key),
                    decode::<FunctionSignature>(value)
                )
            }
            DbTag::StaticMetadataByTarget => {
                format!("{}: {:?}", self.node(key), decode::<StaticMetadata>(value))
            }
            DbTag::UnresolvedCallsByOwner => format!("{}: {:?}", self.node(key), decode::<Vec<super::model::UnresolvedCall>>(value)),
            DbTag::StubOwners => format!("{} named by {}", self.node(key), self.node(value)),
            DbTag::OwnedRowsByCrate => match OwnedRow::decode(value) {
                Ok(row) => format!("{} owns {}", self.node(key), self.owned_row_text(&row)),
                Err(err) => format!("{} owns <undecodable row: {err}>", self.node(key)),
            },
        }
    }

    /// An owned row by the names of what it points at: the key and value
    /// of each table are rendered as that table's own rows are.
    fn owned_row_text(&self, row: &OwnedRow) -> String {
        let key = &row.key[..];
        let dup = row.dup.as_ref().map(|d| &d[..]);
        match (row.tag, dup) {
            (
                DbTag::NodesById | DbTag::SignaturesByTarget | DbTag::StaticMetadataByTarget | DbTag::UnresolvedCallsByOwner,
                None,
            ) => format!("{} {}", row.tag, self.node(key)),
            (DbTag::BindingsById, None) => format!("{} {}", row.tag, self.binding(key)),
            (DbTag::UsagesById, None) => format!("{} {}", row.tag, self.usage(key)),
            (DbTag::BindingsByFromModule | DbTag::BindingsByTarget, Some(dup)) => {
                format!("{} {} → {}", row.tag, self.node(key), self.binding(dup))
            }
            (
                DbTag::UsagesByTarget | DbTag::UsagesByConsumer | DbTag::UsagesByConsumerFunction | DbTag::UsagesByConsumerItem,
                Some(dup),
            ) => format!("{} {} → {}", row.tag, self.node(key), self.usage(dup)),
            (DbTag::ChildrenByParent | DbTag::StubOwners, Some(dup)) => {
                format!("{} {} → {}", row.tag, self.node(key), self.node(dup))
            }
            (tag, dup) => format!("{tag} {} → {:?} <not a row shape>", hex(key), dup.map(hex)),
        }
    }

    fn sides(&self) -> [(&OpenedSnapshot, &RoTx<'_>); 2] {
        [self.left, self.right]
    }

    fn node(&self, id: &[u8]) -> String {
        let Some(id) = as_id(id).map(NodeId) else {
            return format!("<{}-byte key>", id.len());
        };
        self.sides()
            .iter()
            .find_map(|(snap, txn)| snap.dbs.nodes_by_id.get(txn, id.as_bytes()).ok().flatten())
            .map_or_else(
                || format!("<unknown node {}>", &id.to_hex()[..12]),
                |n| n.qualified_name,
            )
    }

    fn binding(&self, id: &[u8]) -> String {
        let Some(id) = as_id(id).map(BindingId) else {
            return format!("<{}-byte binding id>", id.len());
        };
        self.sides()
            .iter()
            .find_map(|(snap, txn)| {
                snap.dbs
                    .bindings_by_id
                    .get(txn, id.as_bytes())
                    .ok()
                    .flatten()
            })
            .map_or_else(
                || format!("<unknown binding {}>", &id.to_hex()[..12]),
                |b| self.binding_text(&b),
            )
    }

    fn usage(&self, id: &[u8]) -> String {
        let Some(id) = as_id(id).map(UsageId) else {
            return format!("<{}-byte usage id>", id.len());
        };
        self.sides()
            .iter()
            .find_map(|(snap, txn)| snap.dbs.usages_by_id.get(txn, id.as_bytes()).ok().flatten())
            .map_or_else(
                || format!("<unknown usage {}>", &id.to_hex()[..12]),
                |u| self.usage_text(&u),
            )
    }

    fn binding_text(&self, b: &Binding) -> String {
        format!(
            "{} :: {} ({:?}) ⇒ {} [{:?}, {:?}, explicit_pub_use={}]",
            self.node(b.from_module.as_bytes()),
            b.visible_name,
            b.namespace,
            self.node(b.target.as_bytes()),
            b.kind,
            b.visibility,
            b.is_explicit_pub_use,
        )
    }

    fn usage_text(&self, u: &Usage) -> String {
        let consumer = match u.consumer_function {
            Some(f) => self.node(f.as_bytes()),
            None => self.node(u.consumer_module.as_bytes()),
        };
        format!(
            "{consumer} uses {} at {}:{}..{} ({:?})",
            self.node(u.target.as_bytes()),
            u.file,
            u.start,
            u.end,
            u.category,
        )
    }
}

fn decode<T: serde::de::DeserializeOwned + 'static>(bytes: &[u8]) -> Option<T> {
    SerdeBincode::<T>::bytes_decode(bytes).ok()
}

fn as_id(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::graph::model::ExtractionModel;
    use crate::graph::snapshot::{
        BuildOptions, build_and_persist, open_current, persist_test_model, usage_id_for,
    };
    use crate::graph::storage::{GraphEnvOptions, GraphPaths};
    use crate::graph::test_support::write_two_crate_fixture;
    use crate::graph::{extract, loader};

    /// A full build of `workspace` into a fresh data directory of its own.
    fn build_into(workspace: &Path, data_dir: &Path) -> OpenedSnapshot {
        let opts = BuildOptions {
            force_rebuild: true,
            data_dir_override: Some(data_dir.to_path_buf()),
            ..Default::default()
        };
        let result = build_and_persist(workspace, opts).expect("build_and_persist");
        let paths = GraphPaths::for_workspace_in(data_dir, &result.workspace_root);
        open_current(&paths, GraphEnvOptions::default())
            .expect("open_current")
            .expect("snapshot published")
    }

    fn rows_per_table(rows: &BTreeSet<SnapshotRow>) -> Vec<(DbTag, usize)> {
        DbTag::ALL
            .iter()
            .map(|&tag| (tag, rows.iter().filter(|(t, _, _)| *t == tag).count()))
            .collect()
    }

    /// Two independent loads and extractions of one tree give the same
    /// snapshot. The fixture is chosen so that every compared table has rows —
    /// an empty table would make "equal" mean "nothing to compare".
    #[test]
    fn two_builds_of_one_tree_are_equal() {
        let workspace = tempfile::tempdir().unwrap();
        write_two_crate_fixture(workspace.path());
        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());

        let a = build_into(workspace.path(), data_a.path());
        let b = build_into(workspace.path(), data_b.path());

        // The unknown-table guard reads this inventory; were it empty, the
        // guard would pass on anything.
        let mut inventory = sub_db_names(&a, &a.read_txn().unwrap()).unwrap();
        inventory.sort();
        let mut expected: Vec<String> = DbTag::ALL.iter().map(|t| t.db_name().to_owned()).collect();
        expected.extend(EXCLUDED_TABLES.iter().map(|n| n.to_string()));
        expected.sort();
        assert_eq!(inventory, expected);

        let rows = dump_snapshot(&a).unwrap();
        for (tag, count) in rows_per_table(&rows) {
            assert!(
                count > 0,
                "fixture leaves `{tag}` empty, so equality there proves nothing"
            );
        }
        assert_eq!(rows, dump_snapshot(&b).unwrap());

        let diff = compare_snapshots(&a, &b, 5).unwrap();
        assert!(diff.is_empty(), "two builds of one tree differ:\n{diff}");
    }

    /// One usage struck out of an otherwise identical model shows up in the
    /// usage table and its three indexes, plus the four ownership records of
    /// those rows — and nowhere else, with the row rendered by name.
    #[test]
    fn a_dropped_usage_is_caught_in_exactly_the_usage_tables() {
        let workspace = tempfile::tempdir().unwrap();
        write_two_crate_fixture(workspace.path());
        let loaded = loader::load(workspace.path()).unwrap();
        let model = extract::extract(&loaded);

        // A usage made inside a function, so it is in all three indexes, and
        // with an id no other usage shares, so dropping it removes rows rather
        // than one duplicate of a row that stays.
        let ids: Vec<UsageId> = model.usages.iter().map(usage_id_for).collect();
        let victim = model
            .usages
            .iter()
            .position(|u| {
                u.consumer_function.is_some()
                    && ids.iter().filter(|&&id| id == usage_id_for(u)).count() == 1
            })
            .expect("fixture has a usage inside a function");
        let mut mutant: ExtractionModel = model.clone();
        let dropped = mutant.usages.remove(victim);
        let target_name = model.nodes[&dropped.target].qualified_name.clone();

        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let original =
            persist_test_model(data_a.path(), &model, GraphEnvOptions::default()).unwrap();
        let struck =
            persist_test_model(data_b.path(), &mutant, GraphEnvOptions::default()).unwrap();

        let diff = compare_snapshots(&original, &struck, 5).unwrap();
        assert_eq!(
            diff.differing_tables(),
            vec![
                DbTag::UsagesById,
                DbTag::UsagesByTarget,
                DbTag::UsagesByConsumer,
                DbTag::UsagesByConsumerFunction,
                DbTag::OwnedRowsByCrate,
                DbTag::UsagesByConsumerItem,
            ],
            "unexpected tables in:\n{diff}"
        );
        let rendered = format!("uses {target_name} at {}", dropped.file);
        for table in &diff.tables {
            // The usage row and its four index rows each have one ownership
            // record, all under the consumer's crate.
            let expected = if table.table == DbTag::OwnedRowsByCrate { 5 } else { 1 };
            assert_eq!((table.only_in_left, table.only_in_right), (expected, 0), "{diff}");
            for sample in &table.left_samples {
                assert!(sample.contains(&rendered), "row not rendered by name: {sample}");
            }
        }
    }

    /// The same equality on a real workspace, for the steps that change how a
    /// snapshot is built: `RMC_SNAPSHOT_COMPARE_WORKSPACE=<path>`. Two full
    /// loads one after the other — on a large workspace this is the full peak
    /// twice over in time, so it is never part of the default run.
    #[test]
    #[ignore = "needs RMC_SNAPSHOT_COMPARE_WORKSPACE and two full builds of it"]
    fn two_builds_of_a_real_workspace_are_equal() {
        let workspace = std::env::var("RMC_SNAPSHOT_COMPARE_WORKSPACE")
            .expect("set RMC_SNAPSHOT_COMPARE_WORKSPACE to a workspace root");
        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = build_into(Path::new(&workspace), data_a.path());
        let b = build_into(Path::new(&workspace), data_b.path());
        let diff = compare_snapshots(&a, &b, 20).unwrap();
        assert!(diff.is_empty(), "two builds of {workspace} differ:\n{diff}");
    }
}
