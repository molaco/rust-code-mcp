#!/usr/bin/env python3
"""Copy a coherent Git working tree without a worktree or branch switch.

Two content inventories bracket copying. A concurrent change fails closed;
the caller may retry into a fresh destination. Includes untracked, nonignored
files and dirty edits. Explicit exclusions are recorded. Cargo path dependencies
outside the root must be frozen separately with their relative layout intact.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root)


def inventory(root, exclusions):
    names = set(git(root, "ls-files", "-z", "-co", "--exclude-standard").split(b"\0")) - {b""}
    records = {}
    for raw in sorted(names):
        name = raw.decode()
        if any(name.startswith(prefix) for prefix in exclusions):
            continue
        path = root / name
        if path.is_symlink():
            raise ValueError(f"symlink needs an explicit frozen dependency: {path}")
        if path.is_file():
            records[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return records


def freeze(root, destination, exclusions):
    root = root.resolve()
    if destination.exists():
        raise ValueError("destination must not exist")
    revision = git(root, "rev-parse", "HEAD").decode().strip()
    status = git(root, "status", "--porcelain=v1").decode()
    patch = git(root, "diff", "--binary", "HEAD")
    before = inventory(root, exclusions)
    destination.mkdir(parents=True)
    for name, digest in before.items():
        data = (root / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"source changed while copying {name}; discard copy and retry")
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    if (before != inventory(root, exclusions) or revision != git(root, "rev-parse", "HEAD").decode().strip()
            or patch != git(root, "diff", "--binary", "HEAD") or status != git(root, "status", "--porcelain=v1").decode()):
        raise ValueError("source tree changed while copying; discard copy and retry")
    provenance = {"source": str(root), "revision": revision, "status": status,
                  "excluded_prefixes": exclusions, "files": before,
                  "coverage": "Git tracked and nonignored untracked files; ignored inputs and external dependencies are outside this inventory"}
    destination.with_suffix(".provenance.json").write_text(json.dumps(provenance, indent=2))
    destination.with_suffix(".dirty.patch").write_bytes(patch)
    print(json.dumps({"source": str(root), "files": len(before), "revision": revision,
                      "destination": str(destination), "bytes": sum((destination / n).stat().st_size for n in before)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--exclude-prefix", action="append", default=[])
    args = parser.parse_args()
    freeze(args.source, args.destination, args.exclude_prefix)
