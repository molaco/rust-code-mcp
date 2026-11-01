#!/usr/bin/env python3
"""Check reference-case addresses and source applicability, without scoring labels.

Reads development labels only. Holdout answers are intentionally not opened.
Exit 2 means at least one seed cannot be used with this graph/source pair.
"""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path, PurePosixPath
import re
import subprocess

from graph_aggregate import node_id


RELATIONS = {"cohere", "separate", "contract_for", "orchestrates", "retire_after"}


def validate_corpus(corpus):
    if corpus["schema_version"] != 1:
        raise ValueError("unsupported reference schema")
    if not re.fullmatch(r"[0-9a-f]{40}", corpus["source_revision"]):
        raise ValueError("source revision must be a full Git commit id")
    cases = corpus["cases"]
    ids = [case["id"] for case in cases]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate case id")
    for file in corpus["files"].values():
        path = PurePosixPath(file["path"])
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("source paths must be repository-relative")
    for symbol in corpus["symbols"].values():
        if symbol["file"] not in corpus["files"] or not symbol["anchor"]:
            raise ValueError("unknown source file or empty anchor")
    development = {case["id"] for case in cases if case["split"] == "development"}
    if set(corpus["expectations"]) != development:
        raise ValueError("public expectations must contain exactly development cases")
    used = set()
    for case in cases:
        if case["split"] not in {"development", "holdout"}:
            raise ValueError("unknown case split")
        seeds = set(case["seeds"])
        if not seeds or not seeds <= corpus["symbols"].keys():
            raise ValueError("unknown or empty case seeds")
        used.update(seeds)
        if case["split"] == "development":
            validate_expectation(corpus["expectations"][case["id"]], seeds)
    if used != corpus["symbols"].keys():
        raise ValueError("unused symbol selector")


def validate_expectation(expectation, seeds):
    if not expectation["operations"] or not expectation["status"] or not expectation["reason"]:
        raise ValueError("expectation needs operations, status and reason")
    for relation, left, right in expectation["constraints"]:
        if relation not in RELATIONS or left not in seeds or right not in seeds or left == right:
            raise ValueError("invalid constraint relation or endpoints")


def read_graph(path):
    nodes = {}
    counts = Counter()
    with path.open() as stream:
        metadata = json.loads(next(stream))["metadata"]
        if metadata["export_version"] != 1:
            raise ValueError("unsupported graph export version")
        for line in stream:
            row = json.loads(line)
            counts[row["table"]] += 1
            if row["table"] == "nodes":
                if row["key"] in nodes or row["key"] != node_id(row["record"]["id"]):
                    raise ValueError("duplicate or inconsistent node row")
                nodes[row["key"]] = row["record"]
    if dict(counts) != {key: value for key, value in metadata["table_counts"].items() if value}:
        raise ValueError("incomplete export or unknown table")
    return metadata, nodes


def check(corpus, source_repo, graph_path, graph_sources):
    validate_corpus(corpus)
    metadata, nodes = read_graph(graph_path)
    targets = {
        ident: f"{n['qualified_name']}[{n['crate_target_kind']}]@{n['file']}"
        for ident, n in nodes.items() if n["kind"] == "Crate"
    }
    index = defaultdict(list)
    for ident, node in nodes.items():
        target = targets.get(node_id(node.get("crate_id")))
        index[(target, node.get("file"), node["qualified_name"], node["item_kind"])].append(ident)
    sources = {}
    for alias, file in corpus["files"].items():
        blob = subprocess.check_output(
            ["git", "show", f"{corpus['source_revision']}:{file['path']}"], cwd=source_repo
        )
        frozen = graph_sources / file["path"]
        sources[alias] = (blob, frozen.is_file() and frozen.read_bytes() == blob)
    symbols = {}
    for name, selector in corpus["symbols"].items():
        file = corpus["files"][selector["file"]]
        blob, source_matches = sources[selector["file"]]
        if selector["anchor"].encode() not in blob:
            raise ValueError(f"source anchor missing: {name}")
        matches = index.get((file["target"], file["path"], selector["name"], selector["kind"]), [])
        status = "ready"
        if not matches:
            status = "graph_missing"
        elif len(matches) != 1:
            status = "graph_ambiguous"
        elif not source_matches:
            status = "graph_stale_source"
        symbols[name] = {"status": status, "node_ids": matches, "file_matches_source": source_matches}
    return {
        "corpus_id": corpus["corpus_id"], "source_revision": corpus["source_revision"],
        "graph_id": metadata["manifest"]["graph_id"], "holdout_answers_read": False,
        "symbol_status_counts": dict(Counter(s["status"] for s in symbols.values())),
        "file_count": len(sources), "matching_files": sum(match for _, match in sources.values()),
        "symbols": symbols,
        "cases": {case["id"]: {"split": case["split"],
            "ready": all(symbols[seed]["status"] == "ready" for seed in case["seeds"]),
            "unavailable_seeds": [seed for seed in case["seeds"] if symbols[seed]["status"] != "ready"]}
            for case in corpus["cases"]},
        "scope": "addressability and exact source-file applicability only; no model or label score",
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--source-repo", type=Path, required=True)
    parser.add_argument("--graph", type=Path, required=True)
    parser.add_argument("--graph-sources", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = check(json.loads(args.corpus.read_text()), args.source_repo, args.graph, args.graph_sources)
    args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({k: result[k] for k in ("symbol_status_counts", "file_count", "matching_files", "cases")}))
    raise SystemExit(0 if all(case["ready"] for case in result["cases"].values()) else 2)
