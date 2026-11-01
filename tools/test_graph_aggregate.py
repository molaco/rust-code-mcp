import unittest
from graph_aggregate import Aggregator, aggregate
from pathlib import Path
import json
import tempfile


def nid(n):
    return [n] * 32


def key(n):
    return bytes(nid(n)).hex()


class ProjectionTests(unittest.TestCase):
    def setUp(self):
        self.nodes = {}
        # Type and method have one semantic owner but two lexical modules/files.
        for n, kind, crate, module, file, parent in [
            (1, "Crate", None, None, None, None),
            (2, "Module", 1, 2, "model.rs", 1),
            (3, "Module", 1, 3, "impl.rs", 1),
            (4, "Item", 1, 2, "model.rs", 2),
            (5, "Item", 1, 3, "impl.rs", 4),
            (6, "Crate", None, None, None, None),
            (7, "Module", 6, 7, "impl.rs", 6),
            (8, "Item", 6, 7, "impl.rs", 7),
            (9, "ExternalSymbol", None, None, None, None),
        ]:
            self.nodes[key(n)] = dict(id=nid(n), kind=kind, crate_id=nid(crate) if crate else None,
                declaration_module=nid(module) if module else None, file=file,
                parent_id=nid(parent) if parent else None)

    def usage(self, rowkey, relation="Call", expanded=True, consumer=5, module=3):
        return {"table": "usages", "key": rowkey, "record": dict(target=nid(4),
            consumer_item=nid(consumer), consumer_module=nid(module), file="impl.rs", start=10, end=14,
            relation=relation, expansion={"expanded_span": [1, 2]} if expanded else None)}

    def test_weights_relations_and_raw_evidence(self):
        agg = Aggregator(self.nodes, "module")
        agg.consume(self.usage("u1"))
        agg.consume(self.usage("u2"))
        agg.consume(self.usage("u3", "FunctionReference", False))
        rows = [r for r in agg.rows() if "relation" in r]
        call = next(r for r in rows if r["relation"] == "Call")
        self.assertEqual(json.loads(call["from"])["node_id"], key(3))
        self.assertEqual(json.loads(call["to"])["node_id"], key(2))
        self.assertEqual(call["weights"], dict(observations=2, expanded_observations=2, source_sites=1,
            distinct_symbols=1, distinct_consumers=1, bindings=0, structural_pairs=0))
        self.assertEqual([r["key"] for r in call["evidence"]], ["u1", "u2"])
        self.assertEqual(len(rows), 2)

    def test_physical_file_keeps_target_and_does_not_follow_type_owner(self):
        agg = Aggregator(self.nodes, "file")
        agg.consume(self.usage("u1"))
        agg.consume(self.usage("u2", consumer=8, module=7))
        rows = [r for r in agg.rows() if "relation" in r]
        self.assertEqual(len(rows), 2)
        sources = [json.loads(r["from"]) for r in rows]
        self.assertEqual({s["file"] for s in sources}, {"impl.rs"})
        self.assertEqual({s["cargo_target"] for s in sources}, {key(1), key(6)})
        self.assertEqual(json.loads(agg.scope(key(5)))["file"], "impl.rs")

    def test_bindings_and_unknowns_have_no_invented_sites_or_targets(self):
        agg = Aggregator(self.nodes, "crate")
        for public in [False, True]:
            agg.consume(dict(table="bindings", key=str(public), record=dict(from_module=nid(3), target=nid(9),
                kind="NamedImport", is_explicit_pub_use=public)))
        agg.consume(dict(table="unresolved_calls", key=key(5), record=[{"expression": "f()"}]))
        rows = [r for r in agg.rows() if "relation" in r]
        self.assertEqual({r["relation"] for r in rows}, {"Import", "ReExport"})
        self.assertTrue(all(r["weights"]["source_sites"] == 0 for r in rows))
        self.assertTrue(all(json.loads(r["to"])["kind"] == "Unlocated" for r in rows))
        self.assertEqual(agg.counts["unresolved_observations"], 1)

    def test_truncated_export_is_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            source, dest = Path(root) / "raw.jsonl", Path(root) / "out.jsonl"
            source.write_text(json.dumps({"metadata": {"export_version": 1, "table_counts": {}, "manifest": {
                "graph_id": "g", "node_count": 1, "binding_count": 0, "usage_count": 0}}}) + "\n")
            with self.assertRaisesRegex(ValueError, "incomplete export"):
                aggregate(source, dest, "item")


if __name__ == "__main__":
    unittest.main()
