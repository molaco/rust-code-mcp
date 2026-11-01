#!/usr/bin/env python3
"""Preview explicitly selected physical moves on a frozen model input.

This relabels locations, not Rust syntax or dependencies. It never deletes an
edge, invents a replacement API, or changes semantic owners, modules or targets.
Import sources remain the observed lexical modules: their future rewrites need
a source patch. Unknown locations are separate endpoints, not a shared bucket.
"""
import argparse
from collections import Counter, defaultdict
from pathlib import Path

from graph_experiment_data import USAGE_RELATIONS, dump, load


def file_scope(node, override=None):
    file = override if override is not None else node['file']
    if file is None or node['cargo_target'] is None:
        return ('Unlocated', node['id'])
    return (node['cargo_target'], file)


def select(nodes, rule):
    """Exact selectors; semantic descendants may follow their selected owner."""
    if not rule.get('file') or not rule.get('cargo_target'):
        raise ValueError('a move requires a located file and Cargo target')
    if not rule.get('names'):
        raise ValueError('a move requires explicit qualified names')
    names = set(rule['names'])
    found = {name: [] for name in names}
    for i, node in enumerate(nodes):
        if (node['file'] == rule['file'] and node['cargo_target'] == rule['cargo_target']
                and node['name'] in names):
            found[node['name']].append(i)
    if any(len(hits) != 1 for hits in found.values()):
        raise ValueError(f'nonunique or missing selectors: {found}')
    selected = {hits[0] for hits in found.values()}
    if rule.get('descendants', False):
        by_parent = defaultdict(list)
        for i, node in enumerate(nodes):
            by_parent[node['semantic_owner']].append(i)
        pending = list(selected)
        while pending:
            for i in by_parent[nodes[pending.pop()]['id']]:
                if i not in selected:
                    if (nodes[i]['file'] != rule['file']
                            or nodes[i]['cargo_target'] != rule['cargo_target']):
                        raise ValueError('descendant is outside the declared file/target')
                    selected.add(i)
                    pending.append(i)
    return selected


def summary(edges, before, after, focus, usage, source_moves):
    counts = defaultdict(Counter)
    rows = []
    for index, edge in enumerate(edges):
        s, t, relation = edge['s'], edge['t'], edge['relation']
        if (relation in USAGE_RELATIONS) != usage:
            continue
        if s not in focus and t not in focus and index not in source_moves:
            continue
        source_scope = source_moves.get(index, after[s])
        old_cross, new_cross = before[s] != before[t], source_scope != after[t]
        c = counts[relation]
        c['typed_pairs'] += 1
        c['source_sites'] += edge['source_sites']
        c['bindings'] += edge['bindings']
        c['before_crossing_pairs'] += old_cross
        c['after_crossing_pairs'] += new_cross
        c['new_crossing_pairs'] += new_cross and not old_cross
        c['internalized_pairs'] += old_cross and not new_cross
        rows.append(dict(edge_index=index, before_crossing=old_cross,
                         after_crossing=new_cross, after_source_scope=source_scope,
                         after_target_scope=after[t], **edge))
    return dict(counts={k: dict(v) for k, v in sorted(counts.items())}, edges=rows)


def mutual_file_pairs(edges, scopes, source_moves=None):
    """Direct two-way file dependencies, not a claim of all SCCs or Cargo cycles."""
    source_moves = source_moves or {}
    pairs = {(source_moves.get(i, scopes[e['s']]), scopes[e['t']]) for i, e in enumerate(edges)
             if e['relation'] in USAGE_RELATIONS
             and source_moves.get(i, scopes[e['s']]) != scopes[e['t']]}
    return {tuple(sorted((s, t))) for s, t in pairs if (t, s) in pairs}


def preview(data, spec, sources=None, current=None):
    if spec['graph_id'] != data['graph_id'] or spec['source_revision'] != data['source_revision']:
        raise ValueError('operation specification belongs to another frozen slice')
    nodes = data['nodes']
    before = [file_scope(n) for n in nodes]
    after = list(before)
    moved = set()
    inventory = []
    for rule in spec['moves']:
        selected = select(nodes, rule)
        if moved & selected:
            raise ValueError('overlapping move assignments')
        if not rule['to_file']:
            raise ValueError('destination file is required')
        moved.update(selected)
        for i in selected:
            after[i] = file_scope(nodes[i], rule['to_file'])
    if not moved:
        raise ValueError('empty preview')
    source_moves = {}
    for rule in spec.get('source_edge_moves', []):
        # A module-attributed impl-header reference can travel with an impl,
        # while the module node stays put. Require the complete exact raw pair;
        # partial site moves need reaggregation and are deliberately rejected.
        hits = [i for i, e in enumerate(data['edges'])
                if e['evidence'] == rule['evidence']]
        if len(hits) != 1 or hits[0] in source_moves:
            raise ValueError('source edge relocation needs unique complete evidence')
        index = hits[0]
        edge = data['edges'][index]
        node = nodes[edge['s']]
        if (node['kind'] != 'Module' or node['file'] != rule['file']
                or node['cargo_target'] != rule['cargo_target']):
            raise ValueError('source relocation requires the declared module endpoint')
        if not rule.get('reason') or not rule.get('to_file'):
            raise ValueError('source relocation requires destination and source audit reason')
        source_moves[index] = file_scope(node, rule['to_file'])
    files = sorted({nodes[i]['file'] for i in moved})
    applicability = {}
    if (sources is None) != (current is None):
        raise ValueError('provide both frozen and current source directories')
    if sources is not None:
        # Compare whole files, not offsets or a coincidentally surviving name.
        for file in files:
            path = current / file
            applicability[file] = ('ready' if path.is_file()
                                   and path.read_bytes() == (sources / file).read_bytes()
                                   else 'source_changed')
    for i in sorted(moved):
        inventory.append(dict(index=i, before_file=nodes[i]['file'],
                              after_file=after[i][1], **nodes[i]))
    old_mutual = mutual_file_pairs(data['edges'], before)
    new_mutual = mutual_file_pairs(data['edges'], after, source_moves)
    affected_scopes = {before[i] for i in moved} | {after[i] for i in moved}
    relevant = lambda pairs: sorted(p for p in pairs if any(s in affected_scopes for s in p))
    return dict(schema_version=1, graph_id=data['graph_id'], source_revision=data['source_revision'],
                name=spec['name'], policy=__doc__, applicability=applicability,
                moved=inventory, unresolved_observations=sum(nodes[i]['unresolved'] for i in moved),
                source_edge_moves=spec.get('source_edge_moves', []),
                usage=summary(data['edges'], before, after, moved, True, source_moves),
                imports_observed_not_rewritten=summary(
                    data['edges'], before, after, moved, False, source_moves),
                mutual_usage_file_pairs=dict(before=relevant(old_mutual), after=relevant(new_mutual),
                                             introduced=relevant(new_mutual-old_mutual),
                                             removed=relevant(old_mutual-new_mutual)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('spec', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--sources', type=Path)
    parser.add_argument('--current', type=Path)
    args = parser.parse_args()
    data, spec = load(args.input), load(args.spec)
    scenarios = spec.get('scenarios', [spec])
    if 'scenarios' in spec:
        args.output.mkdir(parents=True, exist_ok=True)
    for scenario in scenarios:
        scenario = dict(scenario, graph_id=spec['graph_id'], source_revision=spec['source_revision'])
        result = preview(data, scenario, args.sources, args.current)
        path = args.output / (scenario['name'] + '.json.gz') if 'scenarios' in spec else args.output
        dump(path, result)
        print({k: result[k] for k in ('name', 'applicability', 'unresolved_observations')})
