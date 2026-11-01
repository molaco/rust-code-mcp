import copy
import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest

from graph_source_plan import render


class SourcePlanTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.text = '//! Analysis.\n\n/// Read α.\npub fn read() {}\n\nfn analyse() {}\n'
        (self.root / 'lib.rs').write_text(self.text)
        self.plan = dict(schema_version=1,
            sources={'lib.rs': hashlib.sha256(self.text.encode()).hexdigest()},
            regions={'reader': dict(path='lib.rs', start='/// Read α.', end='fn analyse()')},
            edits=[dict(path='lib.rs', remove=['reader'], replace=[
                dict(before='//! Analysis.', after='//! Analysis.\n\npub mod files;', count=1)])],
            creates=[dict(path='files.rs', parts=[dict(text='//! Files.\n\n'), dict(region='reader'),
                                                dict(text='// End of codec.\n')])])

    def test_patch_applies_and_keeps_source_and_moved_docs_exact(self):
        patch, outputs = render(self.root, self.plan)
        self.assertEqual((self.root / 'lib.rs').read_text(), self.text)
        subprocess.run(['git', 'apply', '-'], input=patch, text=True, cwd=self.root, check=True)
        for name, text in outputs.items():
            self.assertEqual((self.root / name).read_text(), text)
        self.assertIn('/// Read α.\npub fn read() {}', outputs['files.rs'])
        self.assertNotIn('pub fn read', outputs['lib.rs'])

    def test_stale_and_ambiguous_source_refused(self):
        (self.root / 'lib.rs').write_text(self.text + '// changed\n')
        with self.assertRaisesRegex(ValueError, 'stale source'):
            render(self.root, self.plan)
        (self.root / 'lib.rs').write_text(self.text)
        self.plan['regions']['reader']['start'] = 'fn '
        with self.assertRaisesRegex(ValueError, 'nonunique'):
            render(self.root, self.plan)

    def test_overlapping_regions_and_unmatched_move_refused(self):
        self.plan['regions']['other'] = copy.deepcopy(self.plan['regions']['reader'])
        self.plan['edits'][0]['remove'].append('other')
        with self.assertRaisesRegex(ValueError, 'overlapping'):
            render(self.root, self.plan)
        del self.plan['regions']['other']
        self.plan['edits'][0]['remove'].pop()
        self.plan['creates'][0]['parts'].pop(1)
        with self.assertRaisesRegex(ValueError, 'exactly once'):
            render(self.root, self.plan)

    def test_replacement_count_existing_file_and_escape_refused(self):
        self.plan['edits'][0]['replace'][0]['count'] = 2
        with self.assertRaisesRegex(ValueError, 'multiplicity'):
            render(self.root, self.plan)
        self.plan['edits'][0]['replace'][0]['count'] = 1
        self.plan['creates'][0]['path'] = 'lib.rs'
        with self.assertRaisesRegex(ValueError, 'already exists'):
            render(self.root, self.plan)
        self.plan['creates'][0]['path'] = '../escape.rs'
        with self.assertRaisesRegex(ValueError, 'relative'):
            render(self.root, self.plan)

    def test_duplicate_insertion_refused(self):
        self.plan['creates'][0]['parts'].append(dict(region='reader'))
        with self.assertRaisesRegex(ValueError, 'duplicate region insertion'):
            render(self.root, self.plan)


if __name__ == '__main__':
    unittest.main()
