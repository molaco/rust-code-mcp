#!/usr/bin/env python3
"""Prepare a provenance-preserving model input from a complete graph export.

This is a research projection, not another extractor. It reuses Aggregator's
typed observations and exact row references. Layout, semantic owner and target
are independent columns. Unresolved calls are counts, never fabricated edges.
"""
import argparse
from collections import Counter
import gzip
import json
from pathlib import Path

from graph_aggregate import Aggregator, node_id
from graph_reference_check import read_graph


USAGE_RELATIONS = {"Call", "TypeRef", "FunctionReference", "Read", "Write", "Use",
                   "Construct", "Implements", "TraitDispatch"}
IMPORT_RELATIONS = {"Import", "ReExport"}


def dump(path, value):
    opener = gzip.open if str(path).endswith('.gz') else open
    with opener(path, 'wt', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, separators=(',', ':'))
        stream.write('\n')


def load(path):
    opener = gzip.open if str(path).endswith('.gz') else open
    with opener(path, 'rt', encoding='utf-8') as stream:
        return json.load(stream)


def prepare(raw, corpus, applicability, sources, forwarders):
    metadata, raw_nodes = read_graph(raw)
    if applicability['graph_id'] != metadata['manifest']['graph_id']:
        raise ValueError('reference check belongs to another graph')
    if any(s['status'] != 'ready' for s in applicability['symbols'].values()):
        raise ValueError('all reference files must match the graph source slice')
    targets = {k: f"{n['qualified_name']}[{n['crate_target_kind']}]@{n['file']}"
               for k, n in raw_nodes.items() if n['kind'] == 'Crate'}
    nodes = []
    for ident, n in sorted(raw_nodes.items()):
        if n['kind'] not in {'Item', 'Module', 'ExternalSymbol'}:
            continue
        nodes.append(dict(id=ident, name=n['qualified_name'], kind=n['kind'],
                          item_kind=n['item_kind'], file=n['file'], span=n['span'],
                          cargo_target=targets.get(node_id(n.get('crate_id'))),
                          module=node_id(n.get('declaration_module')),
                          semantic_owner=node_id(n.get('parent_id')), unresolved=0))
    indices = {n['id']: i for i, n in enumerate(nodes)}
    agg = Aggregator(raw_nodes, 'item')
    with raw.open() as stream:
        next(stream)
        for line in stream:
            row = json.loads(line)
            agg.consume(row)
            if row['table'] == 'unresolved_calls' and row['key'] in indices:
                nodes[indices[row['key']]]['unresolved'] += len(row['record'])
    edges, exclusions = [], Counter()
    for (source, target, relation), e in sorted(agg.edges.items()):
        if relation not in USAGE_RELATIONS | IMPORT_RELATIONS:
            exclusions[relation] += len(e['evidence'])
            continue
        s, t = json.loads(source)['node_id'], json.loads(target)['node_id']
        if s not in indices or t not in indices:
            exclusions['unmodeled_endpoint'] += len(e['evidence'])
            continue
        edges.append(dict(s=indices[s], t=indices[t], relation=relation,
                          source_sites=len(e['sites']), observations=e['observations'],
                          bindings=e['bindings'], expanded_observations=e['expanded_observations'],
                          evidence=e['evidence']))
    references = {name: indices[sel['node_ids'][0]]
                  for name, sel in applicability['symbols'].items()}
    audited = audit_forwarders(forwarders, nodes, edges, sources)
    return dict(schema_version=1, graph_id=metadata['manifest']['graph_id'],
                source_revision=corpus['source_revision'], raw_metadata=metadata,
                source=str(raw), nodes=nodes, edges=edges, forwarders=audited,
                references=references,
                cases={c['id']: dict(split=c['split'], seeds=c['seeds']) for c in corpus['cases']},
                excluded_relations=dict(exclusions),
                policy='typed item projection; self loops retained in data; external stubs flagged; no consumer clique')


def audit_forwarders(spec, nodes, edges, sources):
    """Only an explicit, source-checked whitelist may create a call path view."""
    result = []
    for item in spec:
        matches = [i for i, n in enumerate(nodes) if n['name'] == item['name']
                   and n['file'] == item['file'] and n['cargo_target'] == item['cargo_target']]
        if len(matches) != 1:
            raise ValueError(f"ambiguous forwarder selector: {item['name']}")
        index = matches[0]
        n = nodes[index]
        start, end = n['span']
        body = (sources / n['file']).read_bytes()[start:end].decode()
        if body != item['source']:
            raise ValueError(f"forwarder source changed: {item['name']}: {body!r}")
        calls = [e for e in edges if e['s'] == index and e['relation'] == 'Call']
        if len(calls) != 1 or nodes[calls[0]['t']]['name'] != item['callee']:
            raise ValueError(f"forwarder does not have the declared unique Call: {item['name']}")
        if any(e['s'] == index and e['relation'] == 'TraitDispatch' for e in edges) or n['unresolved']:
            raise ValueError('a forwarder with unresolved dispatch cannot be bypassed')
        result.append(dict(node=index, callee=calls[0]['t'], evidence=calls[0]['evidence'],
                           source=body, reason=item['reason']))
    return result


def select_view(data, spec):
    """Return typed edges, retaining observations and path provenance on bypass."""
    relations = set(spec['relations'])
    internal = {i for i, n in enumerate(data['nodes']) if n['cargo_target'] is not None}
    edges = [e for e in data['edges'] if e['relation'] in relations and e['s'] != e['t']
             and (spec.get('external', False) or e['s'] in internal and e['t'] in internal)]
    removed = Counter()
    if spec.get('drop_target_min_indegree') is not None:
        consumers = {}
        for e in edges:
            consumers.setdefault(e['t'], set()).add(e['s'])
        hubs = {t for t, s in consumers.items() if len(s) >= spec['drop_target_min_indegree']}
        removed['hub_edges'] = sum(e['t'] in hubs for e in edges)
        removed['hub_targets'] = len(hubs)
        edges = [e for e in edges if e['t'] not in hubs]
    if spec.get('bypass_forwarders'):
        forwards = {f['node']: f for f in data['forwarders']}
        transformed = []
        for edge in edges:
            e = dict(edge)
            path = []
            # Only incoming Call is rewritten. TypeRef, function values, and
            # the wrapper's own outgoing observation remain exactly themselves.
            if e['relation'] == 'Call':
                seen = {e['s']}
                while e['t'] in forwards:
                    if e['t'] in seen:
                        raise ValueError('forwarder cycle in whitelist')
                    seen.add(e['t'])
                    f = forwards[e['t']]
                    path.append(dict(via=e['t'], evidence=f['evidence']))
                    e['t'] = f['callee']
            if path:
                e.update(observed=False, path=path)
                removed['bypassed_calls'] += 1
            if e['s'] != e['t']:
                transformed.append(e)
        edges = transformed
    return edges, dict(removed)


def model_edges(data, spec, omission_seed=None):
    """Omissions remove typed pairs, not runtime executions or guessed facts."""
    import numpy as np
    edges, changes = select_view(data, spec)
    if spec.get('drop_fraction', 0):
        rng = np.random.default_rng(omission_seed)
        mask = rng.random(len(edges)) >= spec['drop_fraction']
        changes['omitted_typed_pairs'] = int((~mask).sum())
        edges = [e for e, keep in zip(edges, mask) if keep]
    endpoints = sorted({e[k] for e in edges for k in ('s', 't')})
    remap = {n: i for i, n in enumerate(endpoints)}
    layers = {r: i for i, r in enumerate(sorted({e['relation'] for e in edges}))}
    weights = [1 if spec['weight'] == 'typed_pair' else
               (e['source_sites'] if e['relation'] in USAGE_RELATIONS else e['bindings']) for e in edges]
    if any(w <= 0 for w in weights):
        raise ValueError('nonpositive model weight')
    return dict(endpoints=endpoints, edges=[(remap[e['s']], remap[e['t']]) for e in edges],
                layers=[layers[e['relation']] for e in edges], layer_names=layers,
                weights=weights, changes=changes, provenance=edges)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('raw', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--corpus', required=True, type=Path)
    parser.add_argument('--applicability', required=True, type=Path)
    parser.add_argument('--sources', required=True, type=Path)
    parser.add_argument('--forwarders', required=True, type=Path)
    args = parser.parse_args()
    data = prepare(args.raw, load(args.corpus), load(args.applicability), args.sources, load(args.forwarders))
    dump(args.output, data)
    print(json.dumps(dict(graph_id=data['graph_id'], nodes=len(data['nodes']), edges=len(data['edges']),
                          forwarders=len(data['forwarders']))))
