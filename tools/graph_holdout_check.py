#!/usr/bin/env python3
"""Compare saved coassignments with separately held-out pair constraints.

Membership is only a diagnostic: especially for role models, coassignment does
not require collocation. This reads answers after fitting; it never fits or
selects a view, weight, scale or operation, and produces no aggregate score.
"""
import argparse
import json
from pathlib import Path


def evaluate(corpus, answers, predictions):
    if answers['corpus_id'] != corpus['corpus_id']:
        raise ValueError('answers belong to another corpus')
    cases = {c['id']: c for c in corpus['cases']}
    expected = {name for name, case in cases.items() if case['split'] == 'holdout'}
    if set(answers['expectations']) != expected:
        raise ValueError('answers must cover exactly the holdout cases')
    results = {}
    for name, answer in answers['expectations'].items():
        case, prediction = cases[name], predictions[name]
        if prediction['split'] != 'holdout':
            raise ValueError('predictions are not held out')
        rows = []
        for relation, left, right in answer['constraints']:
            if relation not in {'cohere', 'separate'}:
                raise ValueError('only pair coassignment constraints are supported')
            if left not in case['seeds'] or right not in case['seeds'] or left == right:
                raise ValueError('constraint endpoints must be distinct case seeds')
            pairs = [p for p in prediction['pairs'] if {p['left'], p['right']} == {left, right}]
            if len(pairs) != 1:
                raise ValueError('missing or ambiguous prediction pair')
            views = {}
            for key, counts in pairs[0]['coassignment'].items():
                observed, together = counts['observed'], counts['together']
                if not 0 <= together <= observed:
                    raise ValueError('invalid observation counts')
                mismatches = together if relation == 'separate' else observed - together
                views[key] = dict(observed=observed, together=together,
                    diagnostic_mismatches=mismatches,
                    status=('unobserved' if not observed else 'matches_all' if not mismatches
                            else 'mismatches_all' if mismatches == observed else 'mixed'))
            rows.append(dict(relation=relation, left=left, right=right, predictions=views))
        results[name] = dict(constraints=rows, expected_operations=answer['operations'],
                             expected_status=answer['status'])
    return dict(corpus_id=corpus['corpus_id'], cases=results,
        policy='All saved views/scales retained; no tuning, aggregate score or inferred source operation. Role coassignment is not file placement.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('corpus', 'answers', 'predictions', 'output'):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    result = evaluate(*(json.loads(p.read_text()) for p in (args.corpus, args.answers, args.predictions)))
    args.output.write_text(json.dumps(result, indent=2) + '\n')
