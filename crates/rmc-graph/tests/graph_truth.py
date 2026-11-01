#!/usr/bin/env python3
"""Compare marked semantic expectations with raw fixture evidence, never rewrite truth."""
import argparse
import collections
import csv
import json
import io
from pathlib import Path
import re


def assess(root, evidence):
    def node(node_id):
        return nodes.get(tuple(node_id)) if node_id is not None else None

    nodes = {tuple(n["id"]): n for n in evidence["nodes"]}
    def target_kind(item):
        return (node(item.get("crate_id")) or {}).get("crate_target_kind") if item else None
    sources = {str(p.relative_to(root)): p.read_text() for p in root.rglob("*.rs")
               if "target" not in p.relative_to(root).parts}
    structure_file = root / "expected-structure.json"
    structure = json.loads(structure_file.read_text()) if structure_file.exists() else {}
    results = []
    for row in csv.DictReader(io.StringIO((root / "expected.tsv").read_text()), delimiter="\t"):
        marker = "/*@" + row["site"] + "*/"
        locations = [(f, text.index(marker) + len(marker))
                     for f, text in sources.items() if marker in text]
        if len(locations) != 1:
            raise ValueError(f"expected one marker {marker}, got {locations}")
        file, offset = locations[0]
        match = re.search(r"\b" + re.escape(row["token"]) + r"\b", sources[file][offset:])
        if not match:
            raise ValueError(f"missing token at {marker}")
        start = len(sources[file][:offset + match.start()].encode())
        end = start + len(row["token"].encode())
        hits = [u for u in evidence["usages"] if u["file"] == file
                and u["start"] <= start and u["end"] >= end]
        observed = []
        for u in hits:
            target = node(u["target"])
            caller = node(u.get("consumer_function"))
            owner = node(u.get("consumer_item")) or caller
            module = node(u["consumer_module"])
            observed.append({"target": target["qualified_name"], "target_id": target["id"],
                             "target_file": target.get("file"),
                             "caller": caller["qualified_name"] if caller else None,
                             "caller_file": caller.get("file") if caller else None,
                             "owner": owner["qualified_name"] if owner else None,
                             "target_kind": target_kind(target), "owner_kind": target_kind(owner),
                             "relation": u.get("relation"),
                             "module": module["qualified_name"], "category": u["category"],
                             "range": [u["start"], u["end"]]})
        issues = []
        status = "observed"
        if row["relation"] == "Import":
            bindings = [b for b in evidence["bindings"] if b["visible_name"] == row["token"]
                        and node(b["target"])["qualified_name"] == row["target"]
                        and node(b["from_module"])["qualified_name"] == row["owner"]
                        and b["kind"] == "NamedImport"]
            if not bindings:
                issues.append("missing_import_binding")
            if hits:
                issues.append("import_also_usage")
        elif row["coverage"] != "included":
            status = "declared_exclusion" if not hits else "unexpected_coverage"
            if row["relation"] == "UnresolvedCall":
                unknowns = [u for u in evidence.get("unresolved_calls", []) if u["file"] == file
                            and u["start"] <= start and u["end"] >= end
                            and (node(u.get("consumer_item")) or {}).get("qualified_name") == row["owner"]]
                status = "observed_unresolved" if unknowns and not hits else "unrepresented_unknown"
                if not unknowns:
                    issues.append("missing_explicit_unknown")
                if hits:
                    issues.append("indirect_call_claimed_resolved")
            elif hits and row["coverage"] in {"cfg-not-all", "cfg-test", "test-target", "example-target"}:
                issues.append("inactive_code_edge")
        else:
            targets = [o["target"] for o in observed]
            expected_target = row["target"].replace("[lib]", "").replace("[bin]", "")
            if expected_target not in targets:
                issues.append("wrong_target" if targets else "missing_edge")
                status = "wrong_target" if targets else "missing_edge"
            expected_owner = row["owner"].replace("[lib]", "").replace("[bin]", "")
            matching = [o for o in observed if o["target"] == expected_target]
            if hits and not any(o["owner"] == expected_owner for o in observed):
                structural = row["site"] in {"field_type", "alias_type", "const_init", "static_fn"}
                issues.append("missing_structural_owner" if structural else "missing_or_wrong_owner")
            if row["owner"].startswith("truth_app[bin]") and hits:
                if not any(o["caller_file"] == "app/src/main.rs" for o in observed):
                    issues.append("target_identity_collision")
            if row["target"].startswith("truth_app[bin]") and hits:
                if not any(o["target_file"] == "app/src/main.rs" for o in observed):
                    issues.append("target_identity_collision")
            for role in ("owner", "target"):
                for kind in ("lib", "bin"):
                    if f"[{kind}]" in row[role] and not any(o[role + "_kind"] == kind for o in matching):
                        issues.append("cargo_target_context_unrepresented")
            if matching and not all(o["relation"] == row["relation"] for o in matching):
                issues.append("relation_unrepresented:" + row["relation"])
        expected_module = structure.get("modules_by_site", {}).get(row["site"])
        if hits and expected_module and not all(o["module"] == expected_module for o in observed):
            issues.append("wrong_consumer_module")
        if row["site"] in structure.get("source_sites", {}):
            expected = structure["source_sites"][row["site"]]
            if len({(u["file"], u["start"], u["end"]) for u in hits}) != expected:
                issues.append("wrong_source_multiplicity")
        if row["site"] in structure.get("expanded_sites", {}):
            expected = structure["expanded_sites"][row["site"]]
            expansions = {json.dumps(u.get("expansion"), sort_keys=True) for u in hits if u.get("expansion")}
            if len(expansions) != expected:
                issues.append("wrong_expanded_multiplicity")
        results.append({**row, "file": file, "start": start, "end": end,
                        "status": status, "issues": issues, "observed": observed})
    counts = collections.Counter(issue for row in results for issue in row["issues"])
    structural_issues = []
    if "method_location" in structure:
        expected = structure["method_location"]
        method = next((n for n in nodes.values() if n["qualified_name"] == expected["qualified_name"]), {})
        for field, actual in (("semantic_owner", node(method.get("parent_id"))),
                              ("physical_module", node(method.get("declaration_module")))):
            if (actual or {}).get("qualified_name") != expected[field]:
                structural_issues.append("wrong_" + field)
        if method.get("file") != expected["physical_file"]:
            structural_issues.append("wrong_physical_file")
    for expected in structure.get("distinct_targets", []):
        if not any(n.get("kind") == "Crate" and n["qualified_name"] == expected["name"]
                   and n.get("crate_target_kind") == expected["kind"] and n.get("file") == expected["root"]
                   for n in nodes.values()):
            structural_issues.append("missing_target:" + expected["kind"])
    counts.update(structural_issues)
    return {"oracle": "handwritten expected.tsv; selected identifier sites, not exhaustive",
            "counts": {"sites": len(results), "included": sum(r["coverage"] == "included" for r in results),
                       "issues": dict(counts), "statuses": dict(collections.Counter(r["status"] for r in results))},
            "raw_counts": {key: len(evidence[key]) for key in ("nodes", "usages", "bindings")},
            "sites": results, "structural_issues": structural_issues}


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("workspace", type=Path)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--strict", action="store_true")
    args = parser.parse_args()
    result = assess(args.workspace, json.loads(args.evidence.read_text()))
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"counts": result["counts"], "raw_counts": result["raw_counts"]}, indent=2))
    if args.strict and result["counts"]["issues"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
