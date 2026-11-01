#!/usr/bin/env python3
"""A separately predeclared Leiden scale extension for an unresolved case.

Uses the exact same model/input code as the main experiment. It does not alter
or refit its SBM results, and retains the reason and parameters before fitting.
"""
import argparse
import hashlib
import itertools
from pathlib import Path
import time

import numpy as np

from graph_experiment_data import dump, load, model_edges
from graph_experiment_report import compare, stable_cores
from graph_model_experiments import leiden_fit, reference_predictions, versions


def probe(data, config, specification, destination):
    destination.mkdir(parents=True, exist_ok=True)
    context = dict(graph_id=data['graph_id'], source_revision=data['source_revision'],
                   configuration=config, probe=specification, versions=versions(),
                   implementation={name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                       for name in ('graph_resolution_probe.py', 'graph_model_experiments.py',
                                    'graph_experiment_data.py', 'graph_experiment_report.py')},
                   holdout_answers_read=False)
    path = destination / 'context.json'
    if path.exists() and load(path) != context:
        raise ValueError('probe context changed')
    dump(path, context)
    comparisons, predictions = [], {}
    for view in specification['views']:
        m = model_edges(data, config['views'][view])
        runs = []
        for seed in config['seeds']:
            path = destination / f'{view}.{seed}.json.gz'
            if path.exists():
                run = load(path)
                if run['context'] != context:
                    raise ValueError('mixed probe results')
            else:
                start = time.monotonic()
                partitions = [p for gamma in specification['resolutions']
                              for p in leiden_fit(m, gamma, seed, config['leiden']['iterations'])]
                run = dict(context=context, view=view, seed=seed, family='leiden',
                           endpoints=m['endpoints'], partitions=partitions,
                           predictions=reference_predictions(data, m['endpoints'], partitions),
                           seconds=round(time.monotonic()-start, 3))
                dump(path, run)
            runs.append(run)
        predictions[view] = [r['predictions'] for r in runs]
        for i, gamma in enumerate(specification['resolutions']):
            vectors = []
            for run in runs:
                v = np.full(len(data['nodes']), -1, dtype=np.int32)
                v[m['endpoints']] = run['partitions'][i]['membership']
                vectors.append(v)
            cores = stable_cores(vectors, [(m['endpoints'][s], m['endpoints'][t]) for s, t in m['edges']], 'leiden')
            comparisons.append(dict(view=view, resolution=gamma,
                blocks=[r['partitions'][i]['blocks'] for r in runs],
                repeat_ari=[compare(a, b) for a, b in itertools.combinations(vectors, 2)],
                cores=cores))
    dump(destination / 'summary.json.gz', dict(context=context, comparisons=comparisons,
                                              predictions=predictions))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('data', 'config', 'specification', 'output'):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    probe(load(args.data), load(args.config), load(args.specification), args.output)
