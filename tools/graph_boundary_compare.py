#!/usr/bin/env python3
"""Compare explicitly named boundaries in complete exports, without NodeId matching.

Rules label physical files within exact Cargo targets. Seeds have separate exact
selectors per slice, so a move/rename or expected retirement must be declared.
No similarity matching, inferred edges or automatic architecture verdicts.
"""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path

from graph_aggregate import node_id
from graph_reference_check import read_graph


def matches(node, target, rule):
    return (target == rule['cargo_target']
            and (node.get('file') in rule.get('files', [])
                 or any((node.get('file') or '').startswith(p)
                        for p in rule.get('file_prefixes', []))))


def summarize(path, spec, side):
    metadata, nodes = read_graph(path)
    targets = {k: f"{n['qualified_name']}[{n['crate_target_kind']}]@{n['file']}"
               for k, n in nodes.items() if n['kind'] == 'Crate'}
    def target(k):
        n = nodes[k]
        return targets.get(k if n['kind'] == 'Crate' else node_id(n.get('crate_id')))

    def address(k):
        n = nodes[k]
        module = node_id(n.get('declaration_module'))
        owner = node_id(n.get('parent_id'))
        return dict(node_id=k, cargo_target=target(k), kind=n['kind'],
                    item_kind=n.get('item_kind'), name=n['qualified_name'],
                    file=n.get('file'), span=n.get('span'),
                    declaration_module=nodes[module]['qualified_name'] if module else None,
                    semantic_owner=nodes[owner]['qualified_name'] if owner else None)

    def label(k, physical_file=None):
        n = nodes[k] if physical_file is None else dict(nodes[k], file=physical_file)
        hits = [name for name, rules in spec['boundaries'].items()
                if any(matches(n, target(k), r) for r in rules)]
        if len(hits) > 1:
            raise ValueError(f'overlapping boundaries: {address(k)}: {hits}')
        return hits[0] if hits else 'External' if n['kind'] == 'ExternalSymbol' else 'Outside'

    labels = {k: label(k) for k in nodes}
    inventory = defaultdict(Counter)
    for k, n in nodes.items():
        if labels[k] not in ('Outside', 'External'):
            inventory[labels[k]][n.get('item_kind') or n['kind']] += 1

    seeds, reverse = {}, defaultdict(list)
    for name, selectors in spec['seeds'].items():
        rule = selectors[side]
        # Absence is checked against the other slice's selector, never guessed.
        selector = rule or selectors['before' if side == 'after' else 'after']
        if selector is None:
            raise ValueError('seed needs at least one selector')
        hits = [k for k, n in nodes.items()
                if target(k) == selector['cargo_target'] and n.get('file') == selector['file']
                and n['qualified_name'] == selector['name']
                and n.get('item_kind') == selector['item_kind']]
        if (rule is None and hits) or (rule is not None and len(hits) != 1):
            raise ValueError(f'{side} seed {name}: expected {0 if rule is None else 1}, got {len(hits)}')
        seeds[name] = dict(address=address(hits[0]) if hits else None,
                           incoming=[], outgoing=[], unresolved=[])
        for k in hits:
            reverse[k].append(name)

    edges, unresolved, relations, reasons = {}, defaultdict(Counter), Counter(), Counter()
    sites, expanded = set(), 0
    with path.open() as stream:
        next(stream)
        for line in stream:
            row = json.loads(line)
            table, key, rec = row['table'], row['key'], row['record']
            if table == 'unresolved_calls':
                unresolved[labels[key]].update(c['reason'] for c in rec)
                reasons.update(c['reason'] for c in rec)
                for seed in reverse[key]:
                    seeds[seed]['unresolved'] = rec
                continue
            if table == 'usages':
                s = node_id(rec.get('consumer_item')) or node_id(rec['consumer_module'])
                t, relation = node_id(rec['target']), rec['relation']
                site = (node_id(rec['consumer_module']), rec['file'], rec['start'], rec['end'])
                sites.add(site)
                expanded += rec.get('expansion') is not None
                relations[relation] += 1
                source_label = label(s, rec['file'])
            elif table == 'bindings':
                s, t = node_id(rec['from_module']), node_id(rec['target'])
                relation = ('Declaration' if rec['kind'] == 'Declared' else
                            'ReExport' if rec['is_explicit_pub_use'] else 'Import')
                site = None
                source_label = labels[s]
            else:
                continue
            evidence = dict(table=table, key=key, target=t)
            if source_label not in ('Outside', 'External') or labels[t] not in ('Outside', 'External'):
                group = (source_label, labels[t], relation)
                e = edges.setdefault(group, dict(observations=0, bindings=0, pairs=set(), sites=set(),
                                                expanded_observations=0, evidence=[]))
                e['pairs'].add((s, t))
                e['evidence'].append(evidence)
                if site is not None:
                    e['observations'] += 1
                    e['sites'].add(site)
                    e['expanded_observations'] += rec.get('expansion') is not None
                else:
                    e['bindings'] += 1
            for seed in reverse[s]:
                seeds[seed]['outgoing'].append(dict(relation=relation, endpoint=address(t),
                                                   usage_file=rec.get('file'), evidence=evidence))
            for seed in reverse[t]:
                seeds[seed]['incoming'].append(dict(relation=relation, endpoint=address(s),
                                                   usage_file=rec.get('file'), evidence=evidence))

    rows = []
    for (s, t, relation), e in sorted(edges.items()):
        e['typed_pairs'], e['source_sites'] = len(e.pop('pairs')), len(e.pop('sites'))
        rows.append(dict(source=s, target=t, relation=relation, **e))
    manifest = metadata['manifest']
    measured = dict(observations=metadata['table_counts']['usages'], source_sites=len(sites),
                    expanded_observations=expanded)
    if manifest.get('usage_counts') != measured:
        raise ValueError('usage units disagree with manifest')
    for table, counter in (('nodes', 'node_count'), ('bindings', 'binding_count'), ('usages', 'usage_count')):
        if metadata['table_counts'][table] != manifest[counter]:
            raise ValueError('table counts disagree with manifest')
    return dict(source=str(path), graph_id=manifest['graph_id'], metadata=metadata,
                boundary_inventory=dict(inventory), edges=rows, seeds=seeds,
                unresolved_by_boundary=dict(unresolved), unresolved_reasons=dict(reasons),
                usage_relations=dict(relations), usage_counts=measured)


def compare(before, after, spec):
    if spec['schema_version'] != 1 or set(spec['boundaries']) & {'Outside', 'External'}:
        raise ValueError('unsupported spec or reserved boundary name')
    for rules in spec['boundaries'].values():
        if not rules or any(not r.get('cargo_target') or not (r.get('files') or r.get('file_prefixes')) for r in rules):
            raise ValueError('boundaries require exact Cargo targets and physical files')
    slices = {side: summarize(path, spec, side) for side, path in [('before', before), ('after', after)]}
    keys = {(e['source'], e['target'], e['relation']) for s in slices.values() for e in s['edges']}
    indices = {side: {(e['source'], e['target'], e['relation']): e for e in s['edges']}
               for side, s in slices.items()}
    delta = []
    for key in sorted(keys):
        weights = {unit: indices['after'].get(key, {}).get(unit, 0) - indices['before'].get(key, {}).get(unit, 0)
                   for unit in ('observations', 'source_sites', 'expanded_observations', 'typed_pairs', 'bindings')}
        if any(weights.values()):
            delta.append(dict(source=key[0], target=key[1], relation=key[2], delta=weights))
    return dict(schema_version=1, specification=spec, slices=slices, boundary_deltas=delta,
                policy='physical file rules, target scoped; typed observations and bindings separate; '
                       'raw row evidence retained; source sites deduplicated per boundary/relation; '
                       'no cross-snapshot NodeId matching; unknown calls are observations, not missing edges; '
                       'deltas include code and extractor/environment effects, not causal attribution')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('before', type=Path)
    p.add_argument('after', type=Path)
    p.add_argument('spec', type=Path)
    p.add_argument('output', type=Path)
    args = p.parse_args()
    args.output.write_text(json.dumps(compare(args.before, args.after, json.loads(args.spec.read_text())),
                                      ensure_ascii=False, indent=2) + '\n')
