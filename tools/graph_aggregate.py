#!/usr/bin/env python3
"""Aggregate canonical graph_export JSONL; retain every contributing row id.

No edges are inferred between consumers of a shared target. Physical files are
scoped by Cargo target; module projection uses declaration_module, never the
semantic parent's module. Missing locations remain explicit unlocated nodes.
"""
import argparse
from collections import Counter
import json
from pathlib import Path

LEVELS = ("item", "file", "module", "crate")


def node_id(value):
    if value is None:
        return None
    if isinstance(value, list) and len(value) == 32:
        return bytes(value).hex()
    raise ValueError("expected a 32-byte NodeId")


class Aggregator:
    def __init__(self, nodes, level):
        if level not in LEVELS:
            raise ValueError("unknown projection")
        self.nodes = nodes
        self.level = level
        self.edges = {}
        self.scopes = {}
        self.counts = Counter()

    def scope(self, ident, file=None):
        node = self.nodes[ident]
        crate = node_id(node.get("crate_id"))
        if node["kind"] == "Crate":
            crate = ident
        if self.level == "item":
            result = {"kind": node["kind"], "node_id": ident}
        elif self.level == "file" and (file or node.get("file")):
            result = {"kind": "File", "file": file or node["file"], "cargo_target": crate}
        elif self.level == "module" and node.get("declaration_module"):
            result = {"kind": "Module", "node_id": node_id(node["declaration_module"])}
        elif self.level == "crate" and crate:
            result = {"kind": "Crate", "node_id": crate}
        else:
            result = {"kind": "Unlocated", "node_id": ident, "projection": self.level}
        key = json.dumps(result, sort_keys=True, separators=(",", ":"))
        self.scopes[key] = result
        return key

    def add(self, relation, source, target, row, site=None, expanded=False, consumer=None):
        key = (source, target, relation)
        if key not in self.edges:
            self.edges[key] = {"evidence": [], "sites": set(), "symbols": set(), "consumers": set(),
                               "observations": 0, "expanded_observations": 0, "bindings": 0, "structural_pairs": 0}
        edge = self.edges[key]
        # A reference resolves back to its canonical raw row; containment's
        # DUP_SORT rows require both ids, not only the parent's key.
        edge["evidence"].append(row)
        if site is not None:
            edge["sites"].add(site)
        edge["symbols"].add(row["target"])
        if consumer is not None:
            edge["consumers"].add(consumer)
        if row["table"] == "usages":
            edge["observations"] += 1
            edge["expanded_observations"] += int(expanded)
        elif row["table"] == "bindings":
            edge["bindings"] += 1
        elif row["table"] == "contains":
            edge["structural_pairs"] += 1

    def consume(self, row):
        table, key, rec = row["table"], row["key"], row["record"]
        self.counts[table] += 1
        if table == "usages":
            module = node_id(rec["consumer_module"])
            consumer = node_id(rec.get("consumer_item")) or module
            target = node_id(rec["target"])
            source_scope = self.scope(consumer, rec["file"]) if self.level == "file" else self.scope(module if self.level == "module" else consumer)
            site = (module, rec["file"], rec["start"], rec["end"])
            self.add(rec["relation"], source_scope, self.scope(target),
                     {"table": table, "key": key, "target": target}, site, rec.get("expansion") is not None, consumer)
        elif table == "bindings":
            source, target = node_id(rec["from_module"]), node_id(rec["target"])
            relation = "Declaration" if rec["kind"] == "Declared" else "ReExport" if rec["is_explicit_pub_use"] else "Import"
            self.add(relation, self.scope(source), self.scope(target), {"table": table, "key": key, "target": target}, consumer=source)
        elif table == "contains":
            child = node_id(rec)
            self.add("Contains", self.scope(key), self.scope(child), {"table": table, "key": key, "target": child}, consumer=key)
        elif table == "unresolved_calls":
            self.counts["unresolved_observations"] += len(rec)
            # Never fabricate a target. Full evidence remains in the raw file.

    def rows(self):
        for key, scope in sorted(self.scopes.items()):
            yield {"scope_id": key, "scope": scope}
        for (source, target, relation), edge in sorted(self.edges.items()):
            weights = {k: edge[k] for k in ("observations", "expanded_observations", "bindings", "structural_pairs")}
            weights.update(source_sites=len(edge["sites"]), distinct_symbols=len(edge["symbols"]), distinct_consumers=len(edge["consumers"]))
            yield {"from": source, "to": target, "relation": relation, "weights": weights, "evidence": edge["evidence"]}


def aggregate(source, destination, level):
    nodes = {}
    with source.open() as stream:
        metadata = json.loads(next(stream))["metadata"]
        if metadata["export_version"] != 1:
            raise ValueError("unsupported graph export version")
        for line in stream:
            row = json.loads(line)
            if row["table"] == "nodes":
                if row["key"] in nodes:
                    raise ValueError("duplicate node row")
                nodes[row["key"]] = row["record"]
    agg = Aggregator(nodes, level)
    # Enumerate isolated nodes too; missing edges never erase an item.
    for ident in nodes:
        agg.scope(ident)
    with source.open() as stream:
        next(stream)
        for line in stream:
            agg.consume(json.loads(line))
    manifest = metadata["manifest"]
    for table, expected in metadata["table_counts"].items():
        if agg.counts[table] != expected:
            raise ValueError(f"incomplete export: {table}: {agg.counts[table]} != {expected}")
    for table, counter in (("nodes", "node_count"), ("bindings", "binding_count"), ("usages", "usage_count")):
        if agg.counts[table] != manifest[counter]:
            raise ValueError(f"incomplete export: {table}: {agg.counts[table]} != {manifest[counter]}")
    output_metadata = {"projection_version": 1, "level": level, "source": str(source),
                       "graph_id": manifest["graph_id"], "raw_metadata": metadata,
                       "counts": dict(agg.counts), "scope_count": len(agg.scopes), "edge_count": len(agg.edges),
                       "policy": "directed, typed, self-loops retained; source locations/expansions in raw evidence; files scoped by Cargo target; declaration modules independent of semantic owners",
                       "units": "observations/source_sites/expanded_observations apply only to usages; bindings are resolved namespace rows; structural_pairs are containment pairs; distinct_symbols/consumers are unique endpoint ids, never runtime counts"}
    with destination.open("w") as stream:
        stream.write(json.dumps({"metadata": output_metadata}, ensure_ascii=False) + "\n")
        for row in agg.rows():
            stream.write(json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n")
    return output_metadata


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--level", choices=LEVELS, required=True)
    args = parser.parse_args()
    result = aggregate(args.source, args.destination, args.level)
    print(json.dumps({k: result[k] for k in ("level", "counts", "scope_count", "edge_count")}))
