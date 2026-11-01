//! Lossless canonical graph rows. Secondary indexes and mutable embedding
//! caches are deliberately excluded; no codemap or query cap is involved.
use std::ops::Bound;
use anyhow::{Result, bail, Context};
use heed::{Database, types::Bytes};
use serde::{Serialize, Deserialize};
use serde_json::{Value, json};
use super::{snapshot::OpenedSnapshot, ids::NodeId, model::*};

pub const EXPORT_VERSION: u32 = 1;
pub const TABLES: [&str; 8] = ["nodes", "bindings", "usages", "contains",
    "unresolved_calls", "signatures", "statics", "stub_owners"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportCursor {
    pub graph_id: String,
    pub table: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportRow {
    pub key: String,
    /// Exact persisted record, including physical and expanded source sites.
    pub record: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportPage {
    pub export_version: u32,
    pub graph_id: String,
    pub table: String,
    pub total_rows: u64,
    pub rows: Vec<ExportRow>,
    pub next_cursor: Option<ExportCursor>,
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.is_ascii() || s.len() % 2 != 0 { bail!("invalid export cursor encoding"); }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i+2], 16).context("invalid export cursor hex")).collect()
}

fn database(s: &OpenedSnapshot, table: &str) -> Result<Database<Bytes, Bytes>> {
    let d = &s.dbs;
    Ok(match table {
        "nodes" => d.nodes_by_id.remap_types(),
        "bindings" => d.bindings_by_id.remap_types(),
        "usages" => d.usages_by_id.remap_types(),
        "contains" => d.children_by_parent,
        "unresolved_calls" => d.unresolved_calls_by_owner.remap_types(),
        "signatures" => d.signatures_by_target.remap_types(),
        "statics" => d.static_metadata_by_target.remap_types(),
        "stub_owners" => d.stub_owners,
        _ => bail!("unknown export table {table}; expected one of {TABLES:?}"),
    })
}

fn record(table: &str, bytes: &[u8]) -> Result<Value> {
    macro_rules! decode { ($ty:ty) => { serde_json::to_value(bincode::deserialize::<$ty>(bytes)?)? }; }
    Ok(match table {
        "nodes" => decode!(Node),
        "bindings" => decode!(Binding),
        "usages" => decode!(Usage),
        "unresolved_calls" => decode!(Vec<UnresolvedCall>),
        "signatures" => decode!(FunctionSignature),
        "statics" => decode!(StaticMetadata),
        "contains" | "stub_owners" => json!(NodeId(bytes.try_into().context("invalid graph id length")?)),
        _ => bail!("unknown export table {table}"),
    })
}

/// Keyset pagination uses (key,value), including DUP_SORT containment rows.
/// A new CURRENT is a hard error for an old cursor, never a partial success.
pub fn export_page(snap: &OpenedSnapshot, table: &str, cursor: Option<&ExportCursor>, limit: usize) -> Result<ExportPage> {
    if !(1..=10000).contains(&limit) { bail!("export limit must be 1..=10000"); }
    let db = database(snap, table)?;
    let after = cursor.map(|c| -> Result<_> {
        if c.graph_id != snap.manifest.graph_id || c.table != table { bail!("export cursor belongs to a different graph or table; restart export"); }
        Ok((unhex(&c.key)?, unhex(&c.value)?))
    }).transpose()?;
    let txn = snap.read_txn()?;
    let bounds = (after.as_ref().map_or(Bound::Unbounded, |(k, _)| Bound::Included(k.as_slice())), Bound::Unbounded);
    let mut rows = Vec::new();
    let mut last = None;
    let mut more = false;
    for entry in db.range(&txn, &bounds)? {
        let (key, value) = entry?;
        if after.as_ref().is_some_and(|(k, v)| (key, value) <= (k.as_slice(), v.as_slice())) { continue; }
        if rows.len() == limit { more = true; break; }
        rows.push(ExportRow { key: hex(key), record: record(table, value)? });
        last = Some(ExportCursor { graph_id: snap.manifest.graph_id.clone(), table: table.to_owned(), key: hex(key), value: hex(value) });
    }
    Ok(ExportPage { export_version: EXPORT_VERSION, graph_id: snap.manifest.graph_id.clone(),
        table: table.to_owned(), total_rows: db.len(&txn)?, rows,
        next_cursor: if more { last } else { None } })
}

pub fn export_metadata(snap: &OpenedSnapshot) -> Result<Value> {
    let txn = snap.read_txn()?;
    let mut counts = std::collections::BTreeMap::new();
    for table in TABLES { counts.insert(table, database(snap, table)?.len(&txn)?); }
    Ok(json!({"export_version": EXPORT_VERSION, "manifest": snap.manifest, "tables": TABLES, "table_counts": counts,
        "id_encoding": "record NodeIds are arrays of 32 bytes; row keys and cursor bytes are hex",
        "units": {"usages": "persisted static reference observations, not runtime frequency",
            "source_sites": "distinct (consumer_module,file,start,end); module implies Cargo target",
            "expanded_observations": "observations with expansion provenance, subset of usages",
            "bindings": "resolved namespace bindings; includes declared items; not source use token counts",
            "contains": "semantic parent-child pairs; declaration_module and file remain separate",
            "unresolved_calls": "owner rows containing observed calls with no graph target"},
        "coverage": ["lib/bin under requested root; all features; cfg(test)=false",
            "no build-script outputs or proc-macro server; inactive and unexpanded code is absent",
            "external symbols are partial stubs; no points-to analysis",
            "unresolved calls are evidence, not edges to guessed targets",
            "bindings have no source token ranges; use observations carry byte ranges",
            "several modules in one target sharing a physical file may be ambiguous",
            "external dependency contents and environment are not a hermetic fingerprint"],
        "excluded_storage": "secondary indexes, row-ownership bookkeeping, mutable embedding cache",
        "exporter_revision": env!("RMC_GRAPH_REVISION"), "exporter_ra_revision": env!("RMC_RA_REVISION"),
        "exporter_salsa_revision": env!("RMC_SALSA_REVISION")}))
}
