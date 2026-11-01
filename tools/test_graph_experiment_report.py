import unittest

import numpy as np

from graph_experiment_report import compare, layout_key, semantic_rollup, stable_cores


class StabilityTests(unittest.TestCase):
    def test_unknown_locations_do_not_become_a_shared_layout_group(self):
        a = dict(id='a', cargo_target='lib', file=None, module=None)
        b = dict(id='b', cargo_target='lib', file=None, module=None)
        self.assertNotEqual(layout_key(a, 'file'), layout_key(b, 'file'))
        self.assertNotEqual(layout_key(a, 'module'), layout_key(b, 'module'))
        self.assertEqual(layout_key(a, 'cargo_target'), layout_key(b, 'cargo_target'))

    def test_orchestrator_rollup_keeps_method_physical_file_and_original_anchor(self):
        nodes = [dict(id=str(i), name=name, item_kind=kind, kind='Item',
                      semantic_owner=parent, file=file, module=file, cargo_target='lib', unresolved=0)
                 for i, (name, kind, parent, file) in enumerate([
                     ('Core', 'Struct', None, 'model.rs'),
                     ('Core::spawn', 'Method', '0', 'impl.rs'),
                     ('run', 'Function', None, 'run.rs')])]
        data = dict(nodes=nodes, edges=[dict(s=1, t=2, relation='Call')])
        owners, rolled = semantic_rollup(data)
        self.assertEqual(owners, [0, 0, 2])
        self.assertEqual(rolled['0']['outgoing'], {'Call': 1})
        self.assertEqual(rolled['0']['physical_files'], ['impl.rs', 'model.rs'])
        self.assertEqual(data['edges'][0]['s'], 1)
        self.assertEqual(data['nodes'][1]['file'], 'impl.rs')

    def test_single_block_null_does_not_become_a_workspace_core(self):
        result = stable_cores([np.zeros(10, dtype=int)]*3, [(0, 1)], 'sbm')
        self.assertEqual(result['cores'], [])
        self.assertEqual(len(result['uncertain']), 10)

    def test_label_permutations_and_missing_nodes_do_not_look_like_changes(self):
        a, b = np.array([0, 0, 1, 1, -1]), np.array([9, 9, 7, 7, 7])
        self.assertEqual(compare(a, b), dict(observed=4, ari=1.0))

    def test_role_core_can_be_disconnected_but_community_core_cannot(self):
        vectors = [np.array([0, 0, 0, 1, 1, 1, -1]),
                   np.array([5, 5, 5, 8, 8, 8, -1])]
        edges = [(s, t) for s in range(3) for t in range(3, 6)]
        role = stable_cores(vectors, edges, 'sbm')
        self.assertEqual(len(role['cores']), 2)
        self.assertEqual(sum(c['internal_untyped_pairs'] for c in role['cores']), 0)
        self.assertEqual(role['unobserved'], [6])
        community = stable_cores(vectors, edges, 'leiden')
        self.assertEqual(community['cores'], [])
        self.assertEqual(len(community['small_stable']), 6)

    def test_intersection_is_not_called_stable_if_most_of_parent_block_moves(self):
        vectors = [np.array([0, 0, 0, 0, 1, 1, 1, 1]),
                   np.array([0, 0, 0, 1, 1, 1, 1, 0])]
        result = stable_cores(vectors, [(0, 1), (1, 2)], 'sbm')
        self.assertEqual(result['cores'], [])
        self.assertEqual(sorted(result['uncertain']), list(range(8)))

    def test_each_connected_core_must_retain_the_parent_block(self):
        # Both repeated parent blocks are connected through their own fringe.
        # Their 80% unanimous intersection has two disconnected components;
        # each component retains only 40%, so neither is a stable core.
        vectors = [np.array([0]*10 + [1]*2),
                   np.array([0]*8 + [1]*2 + [0]*2)]
        edges = [(0, 1), (1, 2), (2, 3), (4, 5), (5, 6), (6, 7),
                 (3, 8), (8, 4), (8, 9), (3, 10), (10, 4), (10, 11)]
        result = stable_cores(vectors, edges, 'leiden')
        self.assertEqual(result['cores'], [])
        self.assertEqual(sorted(result['uncertain']), list(range(12)))

    def test_connected_core_reports_its_own_retention(self):
        # The intersection retains 90%, its surviving connected core 80%.
        vectors = [np.array([0]*10 + [1]),
                   np.array([0]*9 + [1, 0])]
        edges = list(zip(range(7), range(1, 8)))
        edges += [(3, 9), (9, 8), (3, 10), (10, 8)]
        result = stable_cores(vectors, edges, 'leiden')
        self.assertEqual(len(result['cores']), 1)
        self.assertEqual(result['cores'][0]['members'], list(range(8)))
        self.assertEqual(result['cores'][0]['parent_retention'], 0.8)


if __name__ == '__main__':
    unittest.main()
