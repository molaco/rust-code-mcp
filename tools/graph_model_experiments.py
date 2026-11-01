#!/usr/bin/env python3
"""Run predeclared Leiden and directed degree-corrected layered nested SBM fits.

No label scoring, parameter search, or holdout-answer reading occurs here.
Each fit is saved independently and can be resumed. An existing result must
match the graph, full configuration and library versions before it is reused.
"""
import argparse
from collections import Counter
import itertools
import hashlib
import json
from pathlib import Path
import time

import numpy as np

from graph_experiment_data import IMPORT_RELATIONS, USAGE_RELATIONS, dump, load, model_edges


def versions():
    import graph_tool
    import igraph
    import leidenalg
    return dict(graph_tool=graph_tool.__version__, igraph=igraph.__version__,
                leidenalg=leidenalg.__version__, numpy=np.__version__)


def canonical(labels):
    """Names of blocks are incidental; first-vertex order defines saved labels."""
    names = {}
    return [names.setdefault(int(v), len(names)) for v in labels]


def metrics(edges, weights, labels):
    total = sum(weights)
    within = sum(w for (s, t), w in zip(edges, weights) if labels[s] == labels[t])
    blocks = Counter(labels)
    matrix = Counter()
    for (s, t), w in zip(edges, weights):
        matrix[(labels[s], labels[t])] += w
    return dict(blocks=len(blocks), largest_block=max(blocks.values(), default=0),
                internal_weight_fraction=within/total if total else None,
                block_edges=[[s, t, w] for (s, t), w in sorted(matrix.items())])


def leiden_fit(m, gamma, seed, iterations):
    import igraph as ig
    import leidenalg as la
    graph = ig.Graph(n=len(m['endpoints']), edges=m['edges'], directed=True)
    partition = la.find_partition(graph, la.RBConfigurationVertexPartition,
                                  weights=m['weights'], resolution_parameter=gamma,
                                  n_iterations=iterations, seed=seed)
    labels = canonical(partition.membership)
    return [dict(scale=str(gamma), membership=labels, quality=partition.quality(),
                 **metrics(m['edges'], m['weights'], labels))]


def sbm_fit(m, seed, settings):
    import graph_tool.all as gt
    if not gt.__version__.startswith('2.98 '):
        raise ValueError('SBM experiments require the validated graph-tool 2.98 implementation')
    gt.openmp_set_num_threads(1)
    gt.seed_rng(seed)
    np.random.seed(seed)
    graph = gt.Graph(directed=True)
    graph.add_vertex(len(m['endpoints']))
    weights = graph.new_ep('int64_t')
    layers = graph.new_ep('int')
    graph.add_edge_list([(s, t, w, layer) for (s, t), w, layer in
                         zip(m['edges'], m['weights'], m['layers'])], eprops=[weights, layers])
    model_args = dict(base_type=gt.LayeredBlockState,
                      state_args=dict(ec=layers, eweight=weights, deg_corr=True, layers=True))
    state = gt.minimize_nested_blockmodel_dl(
        graph, state_args=model_args,
        multilevel_mcmc_args=dict(niter=settings['sweep_iterations']))
    null = gt.NestedBlockState(graph, bs=[np.zeros(len(m['endpoints']), dtype='int32'),
                                        np.zeros(1, dtype='int32')], **model_args)
    entropy, null_entropy = float(state.entropy()), float(null.entropy())
    if entropy > null_entropy + 1e-6:
        raise ValueError(f'fitted SBM is worse than its one-block null: {entropy} > {null_entropy}')
    # project every hierarchy level back to original item vertices before
    # canonicalising: intermediate label ids index the next level's vector.
    hierarchy = []
    labels = np.arange(len(m['endpoints']))
    for level, block_map in enumerate(state.get_bs()):
        labels = np.asarray(block_map)[labels]
        members = canonical(labels)
        hierarchy.append(dict(level=level, membership=members,
                              **metrics(m['edges'], m['weights'], members)))
    nontrivial = [h for h in hierarchy if h['blocks'] > 1]
    result = []
    for target in settings['scale_targets']:
        h = min(nontrivial or hierarchy,
                key=lambda h: (abs(np.log(h['blocks']/target)), h['level']))
        result.append(dict(scale=str(target), hierarchy_level=h['level'],
                           entropy_nats=entropy, **h))
    return result, dict(hierarchy_blocks=[h['blocks'] for h in hierarchy],
                        entropy_nats=entropy, null_entropy_nats=null_entropy)


def reference_predictions(data, endpoints, partitions):
    indices = {global_id: local for local, global_id in enumerate(endpoints)}
    predictions = {}
    for case_id, case in data['cases'].items():
        pairs = []
        for left, right in itertools.combinations(case['seeds'], 2):
            s, t = (indices.get(data['references'][name]) for name in (left, right))
            pairs.append(dict(left=left, right=right, observed=s is not None and t is not None,
                              together={p['scale']: (p['membership'][s] == p['membership'][t]
                                         if s is not None and t is not None else None)
                                        for p in partitions}))
        predictions[case_id] = dict(split=case['split'], pairs=pairs)
    return predictions


def run(data, config, output, only_views=None):
    if config['schema_version'] != 1 or len(set(config['seeds'])) != len(config['seeds']):
        raise ValueError('unsupported configuration or duplicate seeds')
    for spec in config['views'].values():
        allowed = {'relations', 'weight', 'external', 'drop_target_min_indegree',
                   'bypass_forwarders', 'drop_fraction'}
        if (set(spec) - allowed or spec['weight'] not in {'typed_pair', 'source_sites'}
                or not set(spec['relations']) <= USAGE_RELATIONS | IMPORT_RELATIONS
                or not 0 <= spec.get('drop_fraction', 0) < 1):
            raise ValueError('invalid view specification')
    output.mkdir(parents=True, exist_ok=True)
    version = versions()
    implementation = {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                      for name in ('graph_model_experiments.py', 'graph_experiment_data.py')}
    context = dict(graph_id=data['graph_id'], source_revision=data['source_revision'],
                   configuration=config, versions=version, implementation=implementation,
                   forwarders=data['forwarders'], references=data['references'],
                   cases=data['cases'], holdout_answers_read=False)
    context_path = output / 'context.json'
    if context_path.exists() and load(context_path) != context:
        raise ValueError('refuse to mix runs with different graph, parameters, or library versions')
    dump(context_path, context)
    for name, spec in config['views'].items():
        if only_views and name not in only_views:
            continue
        seeds = config['seeds']
        for seed in seeds:
            omission_seed = config['omission_seed_offset'] + seed
            m = model_edges(data, spec, omission_seed)
            for family in ('leiden', 'sbm'):
                path = output / f'{name}.{family}.{seed}.json.gz'
                if path.exists():
                    result = load(path)
                    if result['context'] != context or result['view'] != name or result['seed'] != seed:
                        raise ValueError(f'inconsistent saved result: {path}')
                    continue
                started = time.monotonic()
                detail = {}
                if family == 'leiden':
                    partitions = [p for gamma in config['leiden']['resolutions']
                                  for p in leiden_fit(m, gamma, seed, config['leiden']['iterations'])]
                else:
                    partitions, detail = sbm_fit(m, seed, config['sbm'])
                result = dict(context=context, view=name, seed=seed, omission_seed=omission_seed,
                              family=family, endpoints=m['endpoints'],
                              typed_pair_count=len(m['edges']), layer_names=m['layer_names'],
                              weight_sum=sum(m['weights']), changes=m['changes'],
                              partitions=partitions, detail=detail,
                              predictions=reference_predictions(data, m['endpoints'], partitions),
                              seconds=round(time.monotonic()-started, 3))
                dump(path, result)
                print(json.dumps(dict(view=name, family=family, seed=seed, seconds=result['seconds'],
                                      active=len(m['endpoints']), pairs=len(m['edges']),
                                      blocks=[p['blocks'] for p in partitions])), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('data', type=Path)
    parser.add_argument('config', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--views', nargs='+')
    args = parser.parse_args()
    run(load(args.data), load(args.config), args.output, args.views)
