import unittest

from graph_holdout_check import evaluate


class HoldoutTests(unittest.TestCase):
    def setUp(self):
        self.corpus = dict(corpus_id='c', cases=[dict(id='h', split='holdout', seeds=['a', 'b'])])
        self.answers = dict(corpus_id='c', expectations=dict(h=dict(operations=['LeaveAsIs'],
            status='preserve', constraints=[['separate', 'a', 'b']])))
        self.predictions = dict(h=dict(split='holdout', pairs=[dict(left='b', right='a',
            coassignment=dict(calls=dict(observed=0, together=0),
                              typed=dict(observed=3, together=2)))]))

    def test_unknown_is_not_success_and_all_saved_views_remain(self):
        row = evaluate(self.corpus, self.answers, self.predictions)['cases']['h']['constraints'][0]
        self.assertEqual(row['predictions']['calls']['status'], 'unobserved')
        self.assertEqual(row['predictions']['typed']['diagnostic_mismatches'], 2)
        self.assertEqual(row['predictions']['typed']['status'], 'mixed')
        self.answers['expectations']['h']['constraints'][0][0] = 'cohere'
        row = evaluate(self.corpus, self.answers, self.predictions)['cases']['h']['constraints'][0]
        self.assertEqual(row['predictions']['typed']['diagnostic_mismatches'], 1)

    def test_foreign_answers_and_missing_predictions_fail(self):
        self.answers['corpus_id'] = 'other'
        with self.assertRaisesRegex(ValueError, 'another corpus'):
            evaluate(self.corpus, self.answers, self.predictions)
        self.answers['corpus_id'] = 'c'
        self.predictions['h']['pairs'] = []
        with self.assertRaisesRegex(ValueError, 'prediction pair'):
            evaluate(self.corpus, self.answers, self.predictions)

    def test_development_case_cannot_enter_holdout(self):
        self.corpus['cases'][0]['split'] = 'development'
        with self.assertRaisesRegex(ValueError, 'exactly the holdout'):
            evaluate(self.corpus, self.answers, self.predictions)


if __name__ == '__main__':
    unittest.main()
