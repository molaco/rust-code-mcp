"""Guard the assessor against silently accepting missing or misattributed edges."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("graph_truth", Path(__file__).with_name("graph_truth.py"))
TRUTH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TRUTH)


class TruthAssessment(unittest.TestCase):
    def test_target_context_and_expansion_multiplicity_require_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = "fn caller() { /*@call*/leaf(); }"
            (root / "lib.rs").write_text(source)
            (root / "expected.tsv").write_text(
                "site\ttoken\trelation\ttarget\towner\tcoverage\n"
                "call\tleaf\tCall\tdep::leaf\tapp[bin]::caller\tincluded\n")
            (root / "expected-structure.json").write_text(json.dumps({
                "source_sites": {"call": 1}, "expanded_sites": {"call": 2}}))
            nodes = [{"id": [1], "qualified_name": "dep::leaf"},
                     {"id": [2], "qualified_name": "app::caller", "crate_id": [3], "file": "app/src/main.rs"},
                     {"id": [3], "qualified_name": "app", "crate_target_kind": "lib"}]
            start = source.index("leaf")
            usage = {"target": [1], "consumer_item": [2], "consumer_function": [2], "consumer_module": [3],
                     "file": "lib.rs", "start": start, "end": start + 4, "category": "Other", "relation": "Call"}
            raw = {"nodes": nodes, "bindings": [], "usages": [usage]}
            issues = TRUTH.assess(root, raw)["counts"]["issues"]
            self.assertIn("cargo_target_context_unrepresented", issues)
            self.assertIn("wrong_expanded_multiplicity", issues)
            nodes[2]["crate_target_kind"] = "bin"
            raw["usages"] = [{**usage, "expansion": {"expanded_span": [i, i+1]}} for i in (1, 2)]
            self.assertFalse(TRUTH.assess(root, raw)["counts"]["issues"])
            raw["usages"][1]["expansion"] = raw["usages"][0]["expansion"]
            self.assertIn("wrong_expanded_multiplicity", TRUTH.assess(root, raw)["counts"]["issues"])

    def test_missing_target_owner_and_kind_are_independent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text("fn caller() { /*@call*/leaf(); }")
            (root / "expected.tsv").write_text(
                "site\ttoken\trelation\ttarget\towner\tcoverage\n"
                "call\tleaf\tCall\tdep::leaf\tapp::caller\tincluded\n")
            nodes = [{"id": [i], "qualified_name": name} for i, name in
                     [(1, "dep::leaf"), (2, "app::caller"), (3, "app"), (4, "dep::other")]]
            raw = {"nodes": nodes, "bindings": [], "usages": []}
            self.assertEqual(TRUTH.assess(root, raw)["sites"][0]["issues"], ["missing_edge"])
            start = (root / "lib.rs").read_text().index("leaf")
            usage = {"target": [1], "consumer_function": [2], "consumer_module": [3],
                     "file": "lib.rs", "start": start, "end": start + 4, "category": "Other"}
            raw["usages"] = [usage]
            self.assertEqual(TRUTH.assess(root, raw)["sites"][0]["issues"], ["relation_unrepresented:Call"])
            usage["relation"] = "Call"
            self.assertEqual(TRUTH.assess(root, raw)["sites"][0]["issues"], [])
            usage["target"] = [4]
            self.assertEqual(TRUTH.assess(root, raw)["sites"][0]["issues"], ["wrong_target"])
            usage["target"] = [1]
            usage["consumer_function"] = None
            self.assertEqual(TRUTH.assess(root, raw)["sites"][0]["issues"], ["missing_or_wrong_owner"])

    def test_unicode_offsets_are_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = "// é\nfn caller() { /*@call*/leaf(); }"
            (root / "lib.rs").write_text(source)
            (root / "expected.tsv").write_text(
                "site\ttoken\trelation\ttarget\towner\tcoverage\n"
                "call\tleaf\tCall\tdep::leaf\tapp::caller\tincluded\n")
            result = TRUTH.assess(root, {"nodes": [], "bindings": [], "usages": []})
            self.assertEqual(result["sites"][0]["start"], source.encode().index(b"leaf"))

    def test_unknown_call_requires_its_lexical_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = "fn caller() { /*@indirect*/f(); }"
            (root / "lib.rs").write_text(source)
            (root / "expected.tsv").write_text(
                "site\ttoken\trelation\ttarget\towner\tcoverage\n"
                "indirect\tf\tUnresolvedCall\t?\tapp::caller\tpoints-to\n")
            start = source.index("f();")
            raw = {"nodes": [{"id": [1], "qualified_name": "app::caller"}], "bindings": [], "usages": [],
                   "unresolved_calls": [{"file": "lib.rs", "start": start, "end": start + 3, "consumer_item": None}]}
            result = TRUTH.assess(root, raw)["sites"][0]
            self.assertEqual(result["issues"], ["missing_explicit_unknown"])
            raw["unresolved_calls"][0]["consumer_item"] = [1]
            result = TRUTH.assess(root, raw)["sites"][0]
            self.assertEqual(result["issues"], [])
            self.assertEqual(result["status"], "observed_unresolved")


if __name__ == "__main__":
    unittest.main()
