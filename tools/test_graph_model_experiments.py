import copy
import tempfile
from pathlib import Path
import unittest

from graph_experiment_data import audit_forwarders, model_edges, select_view
from graph_model_experiments import leiden_fit, reference_predictions, sbm_fit


def edge(s, t, relation='Call', sites=1):
    return dict(s=s, t=t, relation=relation, source_sites=sites,
                bindings=0, evidence=[dict(table='usages', key=f'{s}-{t}-{relation}')])


class ProjectionTests(unittest.TestCase):
    def setUp(self):
        self.data = dict(nodes=[dict(cargo_target='lib') for _ in range(6)],
                         edges=[edge(0, 1), edge(1, 2), edge(3, 1, 'TypeRef'),
                                edge(4, 1, 'FunctionReference'), edge(0, 0)],
                         forwarders=[dict(node=1, callee=2, evidence=[dict(key='1-2-Call')])])
        self.spec = dict(relations=['Call', 'TypeRef', 'FunctionReference'], weight='typed_pair')

    def test_shared_target_is_not_a_consumer_clique_and_isolates_are_explicit(self):
        model = model_edges(self.data, self.spec)
        self.assertEqual(model['endpoints'], [0, 1, 2, 3, 4])
        self.assertEqual(len(model['edges']), 4)
        self.assertNotIn((3, 4), model['edges'])
        self.assertEqual(set(model['layer_names']), {'Call', 'TypeRef', 'FunctionReference'})

    def test_call_bypass_has_a_path_and_preserves_other_relations_and_raw_data(self):
        before = copy.deepcopy(self.data)
        transformed, count = select_view(self.data, dict(self.spec, bypass_forwarders=True))
        bypass = next(e for e in transformed if e['s'] == 0)
        self.assertEqual((bypass['t'], bypass['path'][0]['via']), (2, 1))
        self.assertFalse(bypass['observed'])
        self.assertEqual(bypass['evidence'], before['edges'][0]['evidence'])
        self.assertEqual(count['bypassed_calls'], 1)
        self.assertEqual(next(e for e in transformed if e['s'] == 4)['t'], 1)
        self.assertEqual(next(e for e in transformed if e['s'] == 3)['t'], 1)
        self.assertEqual(before, self.data)

    def test_omission_is_seeded_and_does_not_invent_edges(self):
        self.data['edges'] = [edge(i // 6, i % 6) for i in range(36)]
        spec = dict(self.spec, drop_fraction=0.3)
        a = model_edges(self.data, spec, 1047)
        self.assertEqual(a, model_edges(self.data, spec, 1047))
        self.assertNotEqual(a['edges'], model_edges(self.data, spec, 1011)['edges'])
        self.assertEqual(len(a['edges']) + a['changes']['omitted_typed_pairs'], 30)

    def test_hub_ablation_keeps_inventory_and_never_erases_a_type_by_size(self):
        self.data['edges'] = [edge(0, 2), edge(1, 2), edge(3, 4)]
        original_nodes = copy.deepcopy(self.data['nodes'])
        model = model_edges(self.data, dict(self.spec, drop_target_min_indegree=2))
        self.assertEqual(model['endpoints'], [3, 4])
        self.assertEqual(model['changes'], dict(hub_edges=2, hub_targets=1))
        self.assertEqual(self.data['nodes'], original_nodes)

    def test_unobserved_reference_pairs_are_null_not_together(self):
        self.data.update(references=dict(left=0, right=5), cases=dict(case=dict(
            split='holdout', seeds=['left', 'right'])))
        prediction = reference_predictions(self.data, [0], [dict(scale='1', membership=[0])])
        self.assertIsNone(prediction['case']['pairs'][0]['together']['1'])

    def test_forwarder_audit_fails_on_changed_source_or_unresolved_call(self):
        with tempfile.TemporaryDirectory() as root:
            sources = Path(root)
            body = 'pub fn f() { g() }'
            (sources / 'lib.rs').write_text(body)
            nodes = [dict(name='f', file='lib.rs', cargo_target='lib', span=[0, len(body)],
                          unresolved=0), dict(name='g')]
            spec = [dict(name='f', file='lib.rs', cargo_target='lib', source=body,
                         callee='g', reason='checked')]
            self.assertEqual(audit_forwarders(spec, nodes, [edge(0, 1)], sources)[0]['callee'], 1)
            nodes[0]['unresolved'] = 1
            with self.assertRaisesRegex(ValueError, 'unresolved'):
                audit_forwarders(spec, nodes, [edge(0, 1)], sources)
            nodes[0]['unresolved'] = 0
            (sources / 'lib.rs').write_text(body.replace('g()', 'h()'))
            with self.assertRaisesRegex(ValueError, 'source changed'):
                audit_forwarders(spec, nodes, [edge(0, 1)], sources)


class ModelTests(unittest.TestCase):
    def test_disassortative_roles_are_recovered_without_within_group_edges(self):
        # Equal in/out degrees cannot themselves explain the two roles. Every
        # edge crosses roles, so an assortative-only solver is insufficient.
        n = 16
        edges = [(a, b) for a in range(n) for b in range(n, 2*n)]
        edges += [(b, a) for a, b in edges[:]]
        model = dict(endpoints=list(range(2*n)), edges=edges,
                     layers=[0]*len(edges), weights=[1]*len(edges))
        partitions, detail = sbm_fit(model, 11, dict(scale_targets=[2, 4], sweep_iterations=1))
        labels = partitions[0]['membership']
        self.assertEqual(len(set(labels[:n])), 1)
        self.assertEqual(len(set(labels[n:])), 1)
        self.assertNotEqual(labels[0], labels[n])
        self.assertEqual(partitions[0]['internal_weight_fraction'], 0)
        self.assertEqual(partitions[0]['blocks'], 2)
        self.assertGreater(detail['entropy_nats'], 0)
        self.assertLess(detail['entropy_nats'], detail['null_entropy_nats'])
        leiden = leiden_fit(model, 1, 11, -1)[0]
        self.assertEqual(len(leiden['membership']), 2*n)
        self.assertGreater(leiden['internal_weight_fraction'], 0)


if __name__ == '__main__':
    unittest.main()
