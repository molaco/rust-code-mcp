#!/usr/bin/env python3
"""Render an explicitly audited source plan as a patch, without editing sources.

This is a byte-pinned text plan, not Rust syntax inference. Regions include their
documentation by explicit markers; all replacements declare their multiplicity.
Compilation and a new graph must check the resulting patch independently.
"""
import argparse
import difflib
import hashlib
import json
from pathlib import Path


def source_path(root, name):
    path = Path(name)
    if path.is_absolute() or not path.parts or '..' in path.parts:
        raise ValueError(f'expected a relative source path: {name}')
    candidate = root / path
    if not candidate.resolve().is_relative_to(root.resolve()):
        raise ValueError(f'path escapes source root: {name}')
    return candidate


def render(root, plan):
    if plan['schema_version'] != 1:
        raise ValueError('unsupported source plan version')
    original = {}
    for name, digest in plan['sources'].items():
        content = source_path(root, name).read_bytes()
        if hashlib.sha256(content).hexdigest() != digest:
            raise ValueError(f'stale source: {name}')
        original[name] = content.decode('utf-8')
    regions = {}
    for name, region in plan.get('regions', {}).items():
        text = original[region['path']]
        start, end = region['start'], region['end']
        if not start or not end or text.count(start) != 1 or text.count(end) != 1:
            raise ValueError(f'nonunique region markers: {name}')
        lo, hi = text.index(start), text.index(end)
        if lo >= hi:
            raise ValueError(f'inverted region: {name}')
        regions[name] = (region['path'], lo, hi, text[lo:hi])
    result = {}
    removed, used = set(), set()
    for edit in plan['edits']:
        name = edit['path']
        if name in result:
            raise ValueError(f'duplicate output: {name}')
        text = original[name]
        spans = []
        for region in edit.get('remove', []):
            path, lo, hi, _ = regions[region]
            if path != name or region in removed:
                raise ValueError(f'invalid region removal: {region}')
            removed.add(region)
            spans.append((lo, hi))
        spans.sort()
        if any(a[1] > b[0] for a, b in zip(spans, spans[1:])):
            raise ValueError(f'overlapping regions: {name}')
        for lo, hi in reversed(spans):
            text = text[:lo] + text[hi:]
        for replacement in edit.get('replace', []):
            before, after, count = (replacement[k] for k in ('before', 'after', 'count'))
            if not before or count < 1 or text.count(before) != count:
                raise ValueError(f'replacement multiplicity differs: {name}: {before!r}')
            text = text.replace(before, after)
        result[name] = text
    for create in plan.get('creates', []):
        name = create['path']
        if name in result or source_path(root, name).exists():
            raise ValueError(f'new file already exists: {name}')
        parts = []
        for part in create['parts']:
            if set(part) == {'region'}:
                region = part['region']
                if region in used:
                    raise ValueError(f'duplicate region insertion: {region}')
                used.add(region)
                parts.append(regions[region][3])
            elif set(part) == {'text'}:
                parts.append(part['text'])
            else:
                raise ValueError('a part requires either text or region')
        result[name] = ''.join(parts)
    if removed != used or used != set(regions):
        raise ValueError('every declared region must move exactly once')
    patch = []
    for name, text in sorted(result.items()):
        source_path(root, name)
        before = original.get(name, '')
        if not text.endswith('\n') or (before and not before.endswith('\n')):
            raise ValueError(f'expected newline-terminated text: {name}')
        patch.extend(difflib.unified_diff(before.splitlines(True), text.splitlines(True),
            fromfile=f'a/{name}' if name in original else '/dev/null', tofile=f'b/{name}'))
    return ''.join(patch), result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('plan', type=Path)
    parser.add_argument('patch', type=Path)
    args = parser.parse_args()
    plan = json.loads(args.plan.read_text())
    patch, outputs = render(args.source, plan)
    # Never allow the only write operation to overwrite an input or a source file.
    if args.patch.resolve().is_relative_to(args.source.resolve()) or args.patch.resolve() == args.plan.resolve():
        raise ValueError('patch must be outside the source tree and plan')
    with args.patch.open('x') as stream:
        stream.write(patch)
    print(json.dumps(dict(files=len(outputs), patch_bytes=len(patch.encode()),
        moved_regions=len(plan.get('regions', {})), source_edited=False)))
