//! Relevance scoring for search-quality evaluation.
//!
//! The unit of ground truth here is a FILE, not a symbol name. An earlier
//! evaluation harness matched `chunk.context.symbol_name.contains(expected)`
//! against names like `new`, `search` and `Result`; such a judgement fires on
//! nearly any result and cannot tell one embedding profile from another.
//! A file path is stable across renames of the items inside it, it is what a
//! human can label by reading the repository, and it is verifiable: a labelled
//! path either exists on disk or the dataset has rotted (see
//! `RelevanceCase::validate`).
//!
//! Everything in this module is pure: no model, no store, no async. That is
//! deliberate — the metric is what a profile comparison rests on, so it has to
//! be gated by the ordinary test suite rather than by a run that needs a GPU,
//! a downloaded model and an API key.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A file path RELATIVE to the indexed repository root, in `/`-separated form.
///
/// Its own type because the neighbouring domain — the `PathBuf` carried by
/// `ChunkContext::file_path` — is ABSOLUTE and machine-specific. Mixing the two
/// silently yields zero matches (an absolute path never equals a relative one),
/// which reads as "the profile found nothing" rather than as a bug. Conversion
/// happens in exactly one place: [`RepoRelPath::from_indexed_path`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoRelPath(pub String);

impl RepoRelPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Strip the repository root off a path coming back from the index.
    ///
    /// Returns `None` when the path lies outside the root — that is not a
    /// "no match", it is a result the dataset can never be labelled against,
    /// and the caller counts it separately.
    pub fn from_indexed_path(
        indexed: &std::path::Path,
        repo_root: &std::path::Path,
    ) -> Option<Self> {
        let relative = indexed.strip_prefix(repo_root).ok()?;
        let text = relative.to_str()?;
        Some(Self(text.replace('\\', "/")))
    }
}

/// One labelled query: the question, and the files that answer it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevanceCase {
    /// The query text, as a user would type it into search.
    pub query: String,
    /// Files a good answer must surface. Labelled by a human reading the repo.
    pub expected_files: Vec<RepoRelPath>,
    /// Free-form note on WHY these files: keeps a later reader from having to
    /// re-derive the judgement, and makes a stale label visible as a lie.
    #[serde(default)]
    pub rationale: String,
}

impl RelevanceCase {
    /// Reject a case that cannot produce a meaningful score.
    ///
    /// An empty `expected_files` would make recall `0/0`. Scoring it as 0.0
    /// would drag the average down as if the profile had failed, when in fact
    /// nothing was asked; scoring it as 1.0 would inflate it. Neither is
    /// honest, so an unlabelled case is a dataset error.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("relevance case has an empty query".to_string());
        }
        if self.expected_files.is_empty() {
            return Err(format!(
                "relevance case `{}` has no expected files; an unlabelled case cannot be scored",
                self.query
            ));
        }
        Ok(())
    }
}

/// Score of a single query against one ranked result list.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseScore {
    /// Fraction of expected files present among the first `k` RESULTS.
    ///
    /// `k` counts results, not distinct files, because that is what the user
    /// scrolls through: ten chunks of one file are ten results and one file.
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    /// `1 / rank` of the first result whose file is expected; 0.0 if none.
    pub reciprocal_rank: f64,
    /// Whether ANY expected file appeared anywhere in the ranked list.
    pub any_hit: bool,
}

/// Score one query. `ranked` is the result list, already folded to file paths,
/// in rank order and WITHOUT de-duplication.
pub fn score_case(case: &RelevanceCase, ranked: &[RepoRelPath]) -> Result<CaseScore, String> {
    case.validate()?;
    let expected: HashSet<&RepoRelPath> = case.expected_files.iter().collect();

    let recall_at = |k: usize| -> f64 {
        // Distinct expected files seen in the first k results — a file that
        // occupies several of the top slots must not count more than once.
        let found: HashSet<&RepoRelPath> = ranked
            .iter()
            .take(k)
            .filter(|path| expected.contains(path))
            .collect();
        found.len() as f64 / expected.len() as f64
    };

    let first_hit = ranked.iter().position(|path| expected.contains(path));

    Ok(CaseScore {
        recall_at_5: recall_at(5),
        recall_at_10: recall_at(10),
        reciprocal_rank: first_hit.map(|pos| 1.0 / (pos + 1) as f64).unwrap_or(0.0),
        any_hit: first_hit.is_some(),
    })
}

/// Aggregate over a whole dataset, carrying its own coverage.
///
/// The coverage fields are not decoration: a mean of 0.31 means one thing when
/// every query contributed and another when a third of them returned nothing at
/// all. A number that does not say how much it looked at cannot be compared
/// across profiles.
#[derive(Debug, Clone, PartialEq)]
pub struct RelevanceSummary {
    /// Queries actually scored.
    pub queries: usize,
    /// Of those, how many found NO expected file anywhere in the ranked list.
    pub queries_without_hit: usize,
    /// Results that fell outside the repository root and could not be judged.
    pub unjudgeable_results: usize,
    pub mean_recall_at_5: f64,
    pub mean_recall_at_10: f64,
    pub mrr: f64,
}

impl RelevanceSummary {
    /// Fold per-query scores into the dataset-level summary.
    ///
    /// `unjudgeable_results` is threaded in from the caller (only it sees the
    /// raw paths) so that the summary can never claim a coverage it did not
    /// measure.
    pub fn from_scores(scores: &[CaseScore], unjudgeable_results: usize) -> Self {
        let queries = scores.len();
        if queries == 0 {
            return Self {
                queries: 0,
                queries_without_hit: 0,
                unjudgeable_results,
                mean_recall_at_5: 0.0,
                mean_recall_at_10: 0.0,
                mrr: 0.0,
            };
        }

        let n = queries as f64;
        // Queries that missed stay in the denominator: dropping them would
        // report the score of the queries that happened to work.
        Self {
            queries,
            queries_without_hit: scores.iter().filter(|s| !s.any_hit).count(),
            unjudgeable_results,
            mean_recall_at_5: scores.iter().map(|s| s.recall_at_5).sum::<f64>() / n,
            mean_recall_at_10: scores.iter().map(|s| s.recall_at_10).sum::<f64>() / n,
            mrr: scores.iter().map(|s| s.reciprocal_rank).sum::<f64>() / n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn paths(items: &[&str]) -> Vec<RepoRelPath> {
        items.iter().map(|p| RepoRelPath::new(*p)).collect()
    }

    fn case(expected: &[&str]) -> RelevanceCase {
        RelevanceCase {
            query: "where does indexing live".to_string(),
            expected_files: paths(expected),
            rationale: String::new(),
        }
    }

    #[test]
    fn perfect_ranking_scores_one() {
        let score = score_case(&case(&["a.rs", "b.rs"]), &paths(&["a.rs", "b.rs", "c.rs"]))
            .unwrap();

        assert_eq!(score.recall_at_5, 1.0);
        assert_eq!(score.reciprocal_rank, 1.0);
        assert!(score.any_hit);
    }

    #[test]
    fn reciprocal_rank_follows_the_first_hit() {
        let score = score_case(&case(&["b.rs"]), &paths(&["x.rs", "y.rs", "b.rs"])).unwrap();

        assert_eq!(score.reciprocal_rank, 1.0 / 3.0);
        assert_eq!(score.recall_at_5, 1.0);
    }

    #[test]
    fn repeated_file_in_the_top_does_not_inflate_recall() {
        // Five chunks of the same file are five results and one file. If the
        // count were per-result, this would read as 5/2 expected files found.
        let score = score_case(
            &case(&["a.rs", "b.rs"]),
            &paths(&["a.rs", "a.rs", "a.rs", "a.rs", "a.rs", "b.rs"]),
        )
        .unwrap();

        assert_eq!(score.recall_at_5, 0.5);
        assert_eq!(score.recall_at_10, 1.0);
    }

    #[test]
    fn cutoff_counts_results_not_distinct_files() {
        // `b.rs` sits at rank 6: inside recall@10, outside recall@5.
        let score = score_case(
            &case(&["b.rs"]),
            &paths(&["x.rs", "x.rs", "x.rs", "x.rs", "x.rs", "b.rs"]),
        )
        .unwrap();

        assert_eq!(score.recall_at_5, 0.0);
        assert_eq!(score.recall_at_10, 1.0);
        assert!(score.any_hit, "a hit past the cutoff is still a hit");
    }

    #[test]
    fn a_miss_is_a_miss_not_a_missing_measurement() {
        let score = score_case(&case(&["b.rs"]), &paths(&["x.rs", "y.rs"])).unwrap();

        assert_eq!(score.recall_at_10, 0.0);
        assert_eq!(score.reciprocal_rank, 0.0);
        assert!(!score.any_hit);
    }

    #[test]
    fn unlabelled_case_is_rejected_rather_than_scored() {
        let err = score_case(&case(&[]), &paths(&["a.rs"])).unwrap_err();
        assert!(err.contains("no expected files"), "{err}");

        let empty_query = RelevanceCase {
            query: "   ".to_string(),
            expected_files: paths(&["a.rs"]),
            rationale: String::new(),
        };
        assert!(empty_query.validate().is_err());
    }

    #[test]
    fn summary_keeps_missed_queries_in_the_denominator() {
        let hit = score_case(&case(&["a.rs"]), &paths(&["a.rs"])).unwrap();
        let miss = score_case(&case(&["a.rs"]), &paths(&["z.rs"])).unwrap();

        let summary = RelevanceSummary::from_scores(&[hit, miss], 0);

        assert_eq!(summary.queries, 2);
        assert_eq!(summary.queries_without_hit, 1);
        // 1.0 and 0.0 averaged — NOT 1.0 from "the queries that worked".
        assert_eq!(summary.mrr, 0.5);
        assert_eq!(summary.mean_recall_at_10, 0.5);
    }

    #[test]
    fn summary_reports_coverage_of_an_empty_run() {
        let summary = RelevanceSummary::from_scores(&[], 7);

        assert_eq!(summary.queries, 0);
        assert_eq!(summary.unjudgeable_results, 7);
        assert_eq!(summary.mrr, 0.0);
    }

    #[test]
    fn indexed_paths_are_made_relative_to_the_repo_root() {
        let root = Path::new("/home/user/repo");

        assert_eq!(
            RepoRelPath::from_indexed_path(Path::new("/home/user/repo/crates/a/src/lib.rs"), root),
            Some(RepoRelPath::new("crates/a/src/lib.rs"))
        );
        // Outside the root: not judgeable, and explicitly not a match.
        assert_eq!(
            RepoRelPath::from_indexed_path(Path::new("/elsewhere/lib.rs"), root),
            None
        );
    }
}
