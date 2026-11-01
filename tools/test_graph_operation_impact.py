import copy
from pathlib import Path
import tempfile
import unittest

from graph_operation_impact import file_scope, preview


def fixture():
    nodes = [dict(id=str(i), name=name, file=file, cargo_target=target,
                  semantic_owner=owner, module='lexical', unresolved=unknown)
             for i, (name, file, target, owner, unknown) in enumerate([
                 ('Core', 'mixed.rs', 'lib', None, 0),
                 ('Core::compute', 'mixed.rs', 'lib', '0', 2),
                 ('adapter', 'mixed.rs', 'lib', None, 0),
                 ('client', 'client.rs', 'lib', None, 0),
                 ('Core', 'mixed.rs', 'bin', None, 0),
             ])]
    edges = [dict(s=s, t=t, relation=r, source_sites=sites, bindings=bindings,
                  observations=sites, evidence=[dict(table=table, key=str(i), target=str(t))])
             for i, (s, t, r, sites, bindings, table) in enumerate([
                 (2, 1, 'Call', 2, 0, 'usages'),
                 (1, 2, 'Call', 1, 0, 'usages'),
                 (3, 1, 'FunctionReference', 1, 0, 'usages'),
                 (2, 0, 'TypeRef', 3, 0, 'usages'),
                 (2, 0, 'Import', 0, 1, 'bindings'),
             ])]
    data = dict(graph_id='g', source_revision='rev', nodes=nodes, edges=edges)
    spec = dict(graph_id='g', source_revision='rev', name='split', moves=[
        dict(file='mixed.rs', cargo_target='lib', names=['Core'], descendants=True,
             to_file='core.rs')])
    return data, spec


class OperationImpactTests(unittest.TestCase):
    def test_split_exposes_mutual_dependency_and_preserves_exact_relations_and_evidence(self):
        data, spec = fixture()
        original = copy.deepcopy(data)
        result = preview(data, spec)
        self.assertEqual(data, original)
        self.assertEqual([n['index'] for n in result['moved']], [0, 1])
        self.assertEqual(result['unresolved_observations'], 2)
        calls = result['usage']['counts']['Call']
        self.assertEqual(calls['before_crossing_pairs'], 0)
        self.assertEqual(calls['after_crossing_pairs'], 2)
        self.assertEqual(calls['source_sites'], 3)
        self.assertEqual(result['usage']['counts']['FunctionReference']['typed_pairs'], 1)
        self.assertEqual(result['usage']['counts']['TypeRef']['source_sites'], 3)
        self.assertNotIn('Import', result['usage']['counts'])
        self.assertEqual(len(result['mutual_usage_file_pairs']['introduced']), 1)
        for row in result['usage']['edges'] + result['imports_observed_not_rewritten']['edges']:
            self.assertEqual(row['evidence'], data['edges'][row['edge_index']]['evidence'])
        self.assertEqual(result['moved'][1]['semantic_owner'], '0')
        self.assertEqual(result['moved'][1]['module'], 'lexical')

    def test_merge_internalizes_edges_without_removing_them(self):
        data, spec = fixture()
        spec['moves'][0].update(file='client.rs', names=['client'], to_file='mixed.rs')
        result = preview(data, spec)
        row = result['usage']['counts']['FunctionReference']
        self.assertEqual(row['internalized_pairs'], 1)
        self.assertEqual(row['typed_pairs'], 1)

    def test_same_name_in_other_target_is_not_moved(self):
        data, spec = fixture()
        self.assertNotIn(4, [n['index'] for n in preview(data, spec)['moved']])

    def test_missing_or_ambiguous_selectors_fail(self):
        data, spec = fixture()
        spec['moves'][0]['names'] = ['absent']
        with self.assertRaisesRegex(ValueError, 'nonunique or missing'):
            preview(data, spec)
        spec['moves'][0]['names'] = ['Core']
        data['nodes'].append(dict(data['nodes'][0], id='duplicate'))
        with self.assertRaisesRegex(ValueError, 'nonunique or missing'):
            preview(data, spec)

    def test_overlapping_moves_and_wrong_snapshot_fail(self):
        data, spec = fixture()
        spec['moves'].append(spec['moves'][0])
        with self.assertRaisesRegex(ValueError, 'overlapping'):
            preview(data, spec)
        spec['graph_id'] = 'other'
        with self.assertRaisesRegex(ValueError, 'another frozen slice'):
            preview(data, spec)

    def test_changed_file_is_explicit_not_silently_current(self):
        data, spec = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            sources, current = Path(tmp)/'frozen', Path(tmp)/'current'
            sources.mkdir()
            current.mkdir()
            (sources/'mixed.rs').write_text('old')
            (current/'mixed.rs').write_text('new')
            result = preview(data, spec, sources, current)
            self.assertEqual(result['applicability'], {'mixed.rs': 'source_changed'})
            (current/'mixed.rs').write_text('old')
            self.assertEqual(preview(data, spec, sources, current)['applicability'],
                             {'mixed.rs': 'ready'})

    def test_unlocated_nodes_are_not_grouped(self):
        a = dict(id='a', file=None, cargo_target=None)
        b = dict(a, id='b')
        self.assertNotEqual(file_scope(a), file_scope(b))

    def test_impl_header_moves_by_exact_evidence_without_moving_lexical_module(self):
        data, spec = fixture()
        data['nodes'][2]['kind'] = 'Module'
        edge = data['edges'][3]
        spec['source_edge_moves'] = [dict(evidence=edge['evidence'], file='mixed.rs',
                                         cargo_target='lib', to_file='core.rs',
                                         reason='the impl header travels with its type')]
        result = preview(data, spec)
        row = next(e for e in result['usage']['edges'] if e['edge_index'] == 3)
        self.assertFalse(row['after_crossing'])
        self.assertEqual(row['s'], 2)
        self.assertNotIn(2, [n['index'] for n in result['moved']])
        spec['source_edge_moves'][0]['evidence'] = [dict(table='usages', key='absent')]
        with self.assertRaisesRegex(ValueError, 'unique complete evidence'):
            preview(data, spec)


if __name__ == '__main__':
    unittest.main()
