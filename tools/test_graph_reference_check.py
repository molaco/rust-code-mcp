import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from graph_reference_check import check, read_graph, validate_corpus


class ReferenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.frozen = self.root / "frozen"
        self.frozen.mkdir()
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        (self.repo / "impl.rs").write_text("fn f() {}\n")
        (self.frozen / "impl.rs").write_text("fn f() {}\n")
        subprocess.run(["git", "add", "impl.rs"], cwd=self.repo, check=True)
        subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
            "commit", "-qm", "Fixture"], cwd=self.repo, check=True)
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.repo, text=True).strip()
        self.corpus = {"schema_version": 1, "corpus_id": "fixture", "source_revision": revision,
            "files": {"impl": {"path": "impl.rs", "target": "same[lib]@lib.rs"}},
            "symbols": {"f": {"file": "impl", "name": "same::T::f", "kind": "Method", "anchor": "fn f()"}},
            "cases": [{"id": "test", "split": "holdout", "seeds": ["f"]}], "expectations": {}}
        self.nodes = []
        # Same file and qualified name in lib and bin: physical ownership and
        # target identity, not the method's semantic parent, select the seed.
        for ident, kind, crate, target_kind, name, file in [
            (1, "Crate", 1, "lib", "same", "lib.rs"),
            (2, "Crate", 2, "bin", "same", "main.rs"),
            (3, "Item", 1, None, "same::T::f", "impl.rs"),
            (4, "Item", 2, None, "same::T::f", "impl.rs"),
        ]:
            self.nodes.append({"id": [ident] * 32, "kind": kind, "crate_id": [crate] * 32,
                "crate_target_kind": target_kind, "qualified_name": name, "file": file,
                "item_kind": "Method" if kind == "Item" else None})
        self.graph = self.root / "graph.jsonl"
        self.write_graph()

    def write_graph(self):
        rows = [{"metadata": {"export_version": 1, "manifest": {"graph_id": "g"},
            "table_counts": {"nodes": len(self.nodes), "usages": 0}}}]
        rows.extend({"table": "nodes", "key": bytes(n["id"]).hex(), "record": n} for n in self.nodes)
        self.graph.write_text("".join(json.dumps(row) + "\n" for row in rows))

    def inspect(self):
        return check(self.corpus, self.repo, self.graph, self.frozen)

    def test_exact_target_and_physical_file_without_reading_holdout(self):
        result = self.inspect()
        self.assertEqual(result["symbols"]["f"]["node_ids"], ["03" * 32])
        self.assertTrue(result["cases"]["test"]["ready"])
        self.assertFalse(result["holdout_answers_read"])

    def test_changed_source_is_not_an_applicable_match(self):
        (self.frozen / "impl.rs").write_text("fn f() { changed(); }\n")
        result = self.inspect()
        self.assertEqual(result["symbols"]["f"]["status"], "graph_stale_source")
        self.assertFalse(result["cases"]["test"]["ready"])

    def test_missing_and_ambiguous_do_not_pick_a_fuzzy_match(self):
        self.corpus["symbols"]["f"]["name"] = "same::f"
        self.assertEqual(self.inspect()["symbols"]["f"]["status"], "graph_missing")
        self.corpus["symbols"]["f"]["name"] = "same::T::f"
        duplicate = copy.deepcopy(self.nodes[2])
        duplicate["id"] = [5] * 32
        self.nodes.append(duplicate)
        self.write_graph()
        self.assertEqual(self.inspect()["symbols"]["f"]["status"], "graph_ambiguous")

    def test_anchor_must_exist_at_declared_revision(self):
        self.corpus["symbols"]["f"]["anchor"] = "fn nonexistent()"
        with self.assertRaisesRegex(ValueError, "source anchor missing"):
            self.inspect()

    def test_holdout_labels_cannot_enter_public_expectations(self):
        self.corpus["expectations"]["test"] = {"operations": ["LeaveAsIs"]}
        with self.assertRaisesRegex(ValueError, "exactly development"):
            validate_corpus(self.corpus)

    def test_truncated_export_is_rejected(self):
        lines = self.graph.read_text().splitlines()
        self.graph.write_text("\n".join(lines[:-1]) + "\n")
        with self.assertRaisesRegex(ValueError, "incomplete export"):
            read_graph(self.graph)

    def test_constraint_endpoint_must_be_a_seed_of_its_case(self):
        self.corpus["cases"][0]["split"] = "development"
        self.corpus["expectations"]["test"] = {"operations": ["LeaveAsIs"], "status": "preserve",
            "reason": "one invariant", "constraints": [["cohere", "f", "unknown"]]}
        with self.assertRaisesRegex(ValueError, "invalid constraint"):
            validate_corpus(self.corpus)


if __name__ == "__main__":
    unittest.main()
