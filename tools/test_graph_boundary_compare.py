import copy
import json
from pathlib import Path
import tempfile
import unittest

from graph_boundary_compare import compare


class BoundaryComparisonTests(unittest.TestCase):
    def fixture(self, root, shift=0, moved=False):
        def nid(i):
            return [i + shift] * 32
        def key(i):
            return bytes(nid(i)).hex()
        rows = []
        # Hand-labelled graph: method belongs to T, body resides in adapter.
        definitions = [
            (1, 'Crate', 'lib.rs', 'p', None, None, None, None, 'lib'),
            (2, 'Module', 'lib.rs', 'p', 1, 2, 1, None, None),
            (3, 'Item', 'lib.rs', 'p::T', 1, 2, 2, 'Struct', None),
            (4, 'Item', 'new.rs' if moved else 'old.rs', 'p::T::f', 1, 2, 3, 'Method', None),
            (5, 'Item', 'lib.rs', 'p::g', 1, 2, 2, 'Function', None),
            (6, 'Crate', 'main.rs', 'p', None, None, None, None, 'bin'),
            (7, 'Module', 'main.rs', 'p', 6, 7, 6, None, None),
            (8, 'Item', 'lib.rs', 'p::g', 6, 7, 7, 'Function', None),
        ]
        for i, kind, file, name, crate, module, parent, item, target in definitions:
            rows.append(dict(table='nodes', key=key(i), record=dict(
                id=nid(i), kind=kind, file=file, qualified_name=name, item_kind=item,
                crate_id=nid(crate) if crate else None, declaration_module=nid(module) if module else None,
                parent_id=nid(parent) if parent else None, span=[10 + shift, 20 + shift], crate_target_kind=target)))
        for i, relation in enumerate(('Call', 'FunctionReference', 'TypeRef')):
            rows.append(dict(table='usages', key=key(10 + i), record=dict(
                consumer_item=nid(4), consumer_module=nid(2), target=nid(3 if relation == 'TypeRef' else 5),
                file='new.rs' if moved else 'old.rs', start=30 + shift, end=31 + shift,
                relation=relation, expansion={} if relation == 'Call' else None)))
        rows.append(dict(table='bindings', key=key(20), record=dict(
            from_module=nid(2), target=nid(5), kind='NamedImport', is_explicit_pub_use=False)))
        rows.append(dict(table='unresolved_calls', key=key(4), record=[dict(reason='IndirectCallable', expression='cb')]))
        counts = {table: sum(r['table'] == table for r in rows) for table in
                  ('nodes', 'usages', 'bindings', 'unresolved_calls', 'contains', 'signatures', 'statics', 'stub_owners')}
        meta = dict(export_version=1, table_counts=counts, manifest=dict(
            graph_id=f'g{shift}', node_count=8, binding_count=1, usage_count=3,
            usage_counts=dict(observations=3, source_sites=1, expanded_observations=1)))
        path = root / f'{shift}.jsonl'
        path.write_text('\n'.join(json.dumps(r) for r in [dict(metadata=meta)] + rows) + '\n')
        return path

    def spec(self):
        target = 'p[lib]@lib.rs'
        seed = dict(cargo_target=target, file='old.rs', name='p::T::f', item_kind='Method')
        return dict(schema_version=1, boundaries={
            'adapter': [dict(cargo_target=target, files=['old.rs', 'new.rs'])],
            'contract': [dict(cargo_target=target, files=['lib.rs'])]},
            seeds={'f': dict(before=seed, after=dict(seed, file='new.rs'))})

    def test_named_boundaries_survive_new_ids_offsets_and_body_move(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = compare(self.fixture(root), self.fixture(root, 30, True), self.spec())
        self.assertEqual(result['boundary_deltas'], [])
        for slice in result['slices'].values():
            edges = {(e['source'], e['target'], e['relation']): e for e in slice['edges']}
            self.assertEqual(edges['adapter', 'contract', 'Call']['observations'], 1)
            self.assertEqual(edges['adapter', 'contract', 'FunctionReference']['observations'], 1)
            self.assertEqual(edges['adapter', 'contract', 'TypeRef']['observations'], 1)
            self.assertEqual(edges['contract', 'contract', 'Import']['source_sites'], 0)
            self.assertEqual(edges['contract', 'contract', 'Import']['bindings'], 1)
            self.assertEqual(slice['boundary_inventory']['contract']['Function'], 1)  # bin excluded
            self.assertEqual(slice['seeds']['f']['address']['semantic_owner'], 'p::T')
            self.assertEqual(slice['unresolved_by_boundary']['adapter'], {'IndirectCallable': 1})
            self.assertEqual(len(slice['seeds']['f']['outgoing']), 3)  # no guessed cb edge
            self.assertTrue(edges['adapter', 'contract', 'Call']['evidence'])

    def test_missing_or_ambiguous_seed_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.fixture(Path(tmp))
            spec = self.spec()
            with self.assertRaisesRegex(ValueError, 'expected 1, got 0'):
                compare(path, path, spec)
            spec['seeds']['f']['after'] = copy.deepcopy(spec['seeds']['f']['before'])
            rows = [json.loads(line) for line in path.read_text().splitlines()]
            duplicate = copy.deepcopy(rows[4])
            duplicate['key'] = '99' * 32
            duplicate['record']['id'] = [153] * 32
            rows.append(duplicate)
            rows[0]['metadata']['table_counts']['nodes'] += 1
            rows[0]['metadata']['manifest']['node_count'] += 1
            path.write_text('\n'.join(map(json.dumps, rows)) + '\n')
            with self.assertRaisesRegex(ValueError, 'expected 1, got 2'):
                compare(path, path, spec)

    def test_explicit_absence_is_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.fixture(Path(tmp))
            spec = self.spec()
            spec['seeds']['f']['after'] = None
            with self.assertRaisesRegex(ValueError, 'expected 0, got 1'):
                compare(path, path, spec)

    def test_overlap_and_truncated_input_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.fixture(Path(tmp))
            spec = self.spec()
            spec['boundaries']['copy'] = spec['boundaries']['adapter']
            with self.assertRaisesRegex(ValueError, 'overlapping boundaries'):
                compare(path, path, spec)
            path.write_text(path.read_text().splitlines()[0] + '\n')
            with self.assertRaisesRegex(ValueError, 'incomplete export'):
                compare(path, path, self.spec())

    def test_usage_file_overrides_consumer_declaration_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.fixture(Path(tmp))
            rows = [json.loads(line) for line in path.read_text().splitlines()]
            for row in rows:
                if row.get('table') == 'usages':
                    row['record']['file'] = 'lib.rs'
            path.write_text('\n'.join(map(json.dumps, rows)) + '\n')
            spec = self.spec()
            spec['seeds']['f']['after'] = spec['seeds']['f']['before']
            result = compare(path, path, spec)
        edges = result['slices']['after']['edges']
        self.assertTrue(all(e['source'] == 'contract' for e in edges))
        self.assertEqual(result['slices']['after']['seeds']['f']['address']['file'], 'old.rs')


if __name__ == '__main__':
    unittest.main()
