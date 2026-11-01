#!/usr/bin/env python3
"""Summarise fits, repeatability, stable cores, roles and uncertain boundaries.

All comparisons are conditional on a named view, scale, and observed endpoints.
This tool never opens architectural answers, and never treats same-role nodes
as a command to place them in one file.
"""
import argparse
from collections import Counter, defaultdict
import itertools
import hashlib
import json
from pathlib import Path
import statistics

import numpy as np
from sklearn.metrics import adjusted_rand_score

from graph_experiment_data import dump, load, model_edges
from graph_model_experiments import canonical, metrics


def expanded(run, partition, size):
    result = np.full(size, -1, dtype=np.int32)
    result[run['endpoints']] = partition['membership']
    return result


def compare(left, right):
    observed = (left >= 0) & (right >= 0)
    count = int(observed.sum())
    return dict(observed=count, ari=float(adjusted_rand_score(left[observed], right[observed]))
                if count >= 2 else None)


def layout_key(node, layout):
    value = node[layout]
    if value is None:
        return ('Unlocated', layout, node['id'])
    return (node['cargo_target'], value)


def stable_cores(vectors, edges, family, retention=0.8):
    """Unanimous membership intersections, with >=80% of every parent block.

    Leiden cores must also be connected in the original view. SBM role cores
    may be disconnected; that absence is explicitly counted. Tiny unanimous
    classes and nodes without observations have their own outcomes.
    """
    from scipy.sparse import coo_matrix
    from scipy.sparse.csgraph import connected_components
    common = np.logical_and.reduce([v >= 0 for v in vectors])
    if any(len(set(v[v >= 0])) <= 1 for v in vectors):
        return dict(cores=[], small_stable=[], uncertain=np.flatnonzero(common).tolist(),
                    unobserved=np.flatnonzero(~common).tolist())
    signatures = defaultdict(list)
    for node in np.flatnonzero(common):
        signatures[tuple(int(v[node]) for v in vectors)].append(int(node))
    sizes = [Counter(v[v >= 0]) for v in vectors]
    adjacency = defaultdict(set)
    for source, target in edges:
        adjacency[source].add(target)
        adjacency[target].add(source)
    cores, small, uncertain = [], [], []
    for signature, members in signatures.items():
        parent_size = max(s[k] for s, k in zip(sizes, signature))
        fraction = len(members)/parent_size
        if fraction < retention:
            uncertain.extend(members)
            continue
        groups = [members]
        if family == 'leiden' and len(members) > 1:
            index = {n: i for i, n in enumerate(members)}
            pairs = [(index[s], index[t]) for s in members for t in adjacency[s] if t in index]
            if pairs:
                row, col = zip(*pairs)
                matrix = coo_matrix((np.ones(len(pairs)), (row, col)), shape=(len(members),)*2)
                _, labels = connected_components(matrix, directed=False)
                groups = [[node for node, label in zip(members, labels) if label == k]
                          for k in sorted(set(labels))]
            else:
                groups = [[n] for n in members]
        for group in groups:
            if len(group) < 3:
                small.extend(group)
                continue
            # Connectivity can split a qualifying intersection into small
            # fractions of its parent blocks. Each published core must satisfy
            # the retention rule itself, rather than inherit the intersection's.
            group_fraction = len(group)/parent_size
            if group_fraction < retention:
                uncertain.extend(group)
                continue
            member_set = set(group)
            internal = sum(t in member_set for s in group for t in adjacency[s])//2
            cores.append(dict(members=group, parent_retention=group_fraction,
                              internal_untyped_pairs=internal))
    cores.sort(key=lambda c: (-len(c['members']), c['members'][0]))
    return dict(cores=cores, small_stable=small, uncertain=uncertain,
                unobserved=np.flatnonzero(~common).tolist())


def profiles(data):
    incoming, outgoing = defaultdict(lambda: defaultdict(set)), defaultdict(lambda: defaultdict(set))
    for e in data['edges']:
        if e['relation'] in {'Import', 'ReExport'} or e['s'] == e['t']:
            continue
        incoming[e['t']][e['relation']].add(e['s'])
        outgoing[e['s']][e['relation']].add(e['t'])
    result = []
    for i, node in enumerate(data['nodes']):
        ins = set().union(*incoming[i].values()) if incoming[i] else set()
        outs = set().union(*outgoing[i].values()) if outgoing[i] else set()
        targets = Counter(data['nodes'][n]['cargo_target'] or 'ExternalSymbol' for n in ins)
        if not ins and not outs:
            role = 'unobserved'
        elif len(ins) >= 4*max(1, len(outs)):
            role = 'fan_in'
        elif len(outs) >= 4*max(1, len(ins)):
            role = 'fan_out'
        else:
            role = 'mixed'
        result.append(dict(index=i, **node, incoming={r: len(s) for r, s in incoming[i].items()},
                           outgoing={r: len(s) for r, s in outgoing[i].items()},
                           distinct_in=len(ins), distinct_out=len(outs),
                           consumer_targets=dict(targets), structural_profile=role))
    return result


def semantic_rollup(data):
    """An explanatory ADT/trait view, separate from all fitted item partitions.

    Struct anchors have no function body. Their methods' fan-out must not be
    attributed to the anchor as an observed Call, or lost when discussing an
    orchestrator. This rollup keeps member identities and every physical file.
    """
    nodes = data['nodes']
    indices = {n['id']: i for i, n in enumerate(nodes)}
    owners = []
    for index, node in enumerate(nodes):
        current, visited, owner = index, set(), index
        while current not in visited:
            visited.add(current)
            n = nodes[current]
            if n['item_kind'] in {'Struct', 'Enum', 'Trait'}:
                owner = current
                break
            parent = indices.get(n['semantic_owner'])
            if parent is None or nodes[parent]['kind'] != 'Item':
                break
            current = parent
        owners.append(owner)
    members = defaultdict(list)
    for index, owner in enumerate(owners):
        members[owner].append(index)
    rolled_edges = []
    for e in data['edges']:
        rolled_edges.append(dict(e, s=owners[e['s']], t=owners[e['t']]))
    rolled = profiles(dict(nodes=nodes, edges=rolled_edges))
    result = {}
    for owner, group in members.items():
        result[str(owner)] = dict(rolled[owner], members=group,
            physical_files=sorted({nodes[i]['file'] for i in group if nodes[i]['file']}),
            lexical_modules=sorted({nodes[i]['module'] for i in group if nodes[i]['module']}),
            unresolved=sum(nodes[i]['unresolved'] for i in group),
            projection='semantic ADT/trait with methods/fields; not observed anchor calls')
    return owners, result


def summarize(data, config, directory, destination):
    destination.mkdir(parents=True, exist_ok=True)
    runs = {}
    context = load(directory / 'context.json')
    if context['graph_id'] != data['graph_id'] or context['configuration'] != config:
        raise ValueError('report input differs from fitted graph/configuration')
    for view, family, seed in itertools.product(config['views'], ('leiden', 'sbm'), config['seeds']):
        run = load(directory / f'{view}.{family}.{seed}.json.gz')
        if run['context'] != context:
            raise ValueError('mixed run contexts')
        runs[view, family, seed] = run
    size = len(data['nodes'])
    vectors = {}
    partitions = {}
    for (view, family, seed), run in runs.items():
        for part in run['partitions']:
            key = (view, family, part['scale'], seed)
            vectors[key] = expanded(run, part, size)
            partitions[key] = part
    summary = []
    core_catalog = {}
    core_counts = []
    for view, family in itertools.product(config['views'], ('leiden', 'sbm')):
        scales = [p['scale'] for p in runs[view, family, config['seeds'][0]]['partitions']]
        m = model_edges(data, config['views'][view], config['omission_seed_offset']+config['seeds'][0])
        original_edges = [(m['endpoints'][s], m['endpoints'][t]) for s, t in m['edges']]
        for scale in scales:
            vs = [vectors[view, family, scale, seed] for seed in config['seeds']]
            parts = [partitions[view, family, scale, seed] for seed in config['seeds']]
            agreements = [compare(a, b) for a, b in itertools.combinations(vs, 2)]
            baseline = [compare(vectors[view, family, scale, seed], vectors['typed', family, scale, seed])
                        for seed in config['seeds']]
            summary.append(dict(view=view, family=family, scale=scale,
                                repetition_scope=('omission mask and model initialization' if
                                    config['views'][view].get('drop_fraction') else 'model initialization'),
                                active=[int((v >= 0).sum()) for v in vs],
                                blocks=[p['blocks'] for p in parts],
                                internal_fraction=[p['internal_weight_fraction'] for p in parts],
                                pairwise_repeat_ari=agreements, versus_typed_ari=baseline))
            # The primary input and each perturbation get separate cores;
            # disagreement between views is not averaged out of existence.
            cores = stable_cores(vs, original_edges, family)
            for core in cores['cores']:
                members = core['members']
                core['files'] = [list(x) for x in Counter(data['nodes'][n]['file'] for n in members).most_common(8)]
                core['examples'] = [data['nodes'][n]['name'] for n in members[:8]]
            core_catalog[f'{view}.{family}.{scale}'] = cores
            core_counts.append(dict(view=view, family=family, scale=scale,
                                    cores=len(cores['cores']), core_nodes=sum(len(c['members']) for c in cores['cores']),
                                    stable_small=len(cores['small_stable']), uncertain=len(cores['uncertain']),
                                    unobserved=len(cores['unobserved']),
                                    disconnected_role_cores=sum(c['internal_untyped_pairs'] == 0 for c in cores['cores'])))
    dump(destination / 'cores.json.gz', core_catalog)
    node_profiles = profiles(data)
    dump(destination / 'node-profiles.json.gz', node_profiles)
    owner_indices, owner_profiles = semantic_rollup(data)
    dump(destination / 'semantic-owner-profiles.json.gz', owner_profiles)
    references = {name: node_profiles[index] for name, index in data['references'].items()}
    pair_predictions = {}
    for case, info in data['cases'].items():
        pair_predictions[case] = dict(split=info['split'], pairs=[])
        for left, right in itertools.combinations(info['seeds'], 2):
            s, t = (data['references'][n] for n in (left, right))
            together = {}
            for view, family in itertools.product(config['views'], ('leiden', 'sbm')):
                scales = [p['scale'] for p in runs[view, family, config['seeds'][0]]['partitions']]
                for scale in scales:
                    vs = [vectors[view, family, scale, seed] for seed in config['seeds']]
                    values = [bool(v[s] == v[t]) for v in vs if v[s] >= 0 and v[t] >= 0]
                    together[f'{view}.{family}.{scale}'] = dict(observed=len(values),
                        together=sum(values), fraction=statistics.mean(values) if values else None)
            pair_predictions[case]['pairs'].append(dict(left=left, right=right, coassignment=together))
    dump(destination / 'reference-predictions.json', pair_predictions)
    typed = model_edges(data, config['views']['typed'])
    layouts = []
    for layout in ('file', 'module', 'cargo_target'):
        values = [layout_key(data['nodes'][n], layout) for n in typed['endpoints']]
        names = {}
        labels = [names.setdefault(value, len(names)) for value in values]
        layouts.append(dict(layout=layout, **metrics(typed['edges'], typed['weights'], labels)))
    result = dict(context=context, fit_count=len(runs), partition_count=len(partitions),
                  report_implementation=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  core_rule=dict(unanimous=True, min_parent_retention=0.8, min_size=3,
                                 leiden_requires_connected=True, single_block_is_uninformative=True),
                  graph_nodes=size, layouts=layouts, comparisons=summary, cores=core_counts,
                  references=references,
                  reference_semantic_owners={name: owner_profiles[str(owner_indices[index])]
                                            for name, index in data['references'].items()},
                  strongest_incoming=sorted(node_profiles, key=lambda p: -p['distinct_in'])[:30],
                  strongest_outgoing=sorted(node_profiles, key=lambda p: -p['distinct_out'])[:30],
                  view_changes={view: runs[view, 'leiden', config['seeds'][0]]['changes'] for view in config['views']},
                  unresolved_observations=sum(n['unresolved'] for n in data['nodes']),
                  policy='cores require unanimous repeated coassignment and >=0.8 retention; no semantic operation is inferred from membership alone')
    dump(destination / 'summary.json', result)
    print(json.dumps(dict(fits=len(runs), partitions=len(partitions), output=str(destination))))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('data', type=Path)
    parser.add_argument('config', type=Path)
    parser.add_argument('runs', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    summarize(load(args.data), load(args.config), args.runs, args.output)
