//! What a BM25 (tantivy) index directory is made of: segments, deleted
//! documents, bytes per file kind, and the bytes nothing references.
//!
//! The question this answers is "is the keyword index proportionate to the
//! data it serves?". A tantivy index that is rewritten file-by-file (delete
//! every chunk of a changed file, add the new ones) keeps the deleted
//! documents inside its segments until a merge drops them, and every merge
//! rewrites the surviving data. With no merge policy that cares about deletes
//! the index grows into mostly-dead segments that the merge threads keep
//! copying — which is what was found on a live daemon: 662 segments, 71 % of
//! documents deleted, 17 GB for under a million live chunks.
//!
//! The probe reads only what is on disk: `meta.json` through tantivy's own
//! parser (the segments the index currently consists of, with their delete
//! counts) and the sizes of the directory's files. It takes no writer lock and
//! opens no reader, so it can be pointed at the index of a running daemon, at
//! an offline copy, or at an index of another schema.
//!
//! Every count here is exact: nothing is sampled or estimated, and nothing
//! decides whether a number is "too big" — that is the caller's knob.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use tantivy::Index;

/// How many of the largest segments [`Bm25IndexStats::largest_segments`] lists.
///
/// The baseline that motivated the probe was read off the six largest
/// segments, which held most of the deleted documents; eight leaves room
/// without turning the report into a dump of all segments.
const LARGEST_SEGMENTS_LISTED: usize = 8;

/// Files of a tantivy directory that belong to the index as a whole rather
/// than to a segment: the commit point, the managed-file list and the lock
/// files of a writer and of a meta reader.
const INDEX_LEVEL_FILES: &[&str] = &[
    "meta.json",
    ".managed.json",
    ".tantivy-writer.lock",
    ".tantivy-meta.lock",
];

/// One segment of the index as `meta.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bm25SegmentStats {
    /// The segment's uuid, as it prefixes its file names.
    pub id: String,
    /// Documents ever written into the segment, deleted ones included.
    pub max_doc: u64,
    pub deleted_docs: u64,
}

/// The state of one BM25 index directory, see the module docs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bm25IndexStats {
    pub path: String,
    /// Segments in the last commit (`meta.json`).
    pub segment_count: usize,
    /// Sum of `max_doc` over the committed segments: live and deleted.
    pub max_doc: u64,
    pub deleted_docs: u64,
    /// `max_doc - deleted_docs`: what a search can actually return.
    pub live_docs: u64,
    /// `deleted_docs / max_doc`; `None` for an index with no documents,
    /// where the ratio means nothing.
    pub deleted_ratio: Option<f64>,
    /// Committed segments by the decimal order of magnitude of their
    /// `max_doc`: key `k` counts segments with `10^k <= max_doc < 10^(k+1)`
    /// (an empty segment counts under 0). Small segments in the hundreds are
    /// what a merge policy that never catches up looks like.
    pub segments_by_doc_decade: BTreeMap<u32, usize>,
    /// The largest committed segments by `max_doc`, largest first.
    pub largest_segments: Vec<Bm25SegmentStats>,
    /// Bytes of the files the committed segments consist of, by extension
    /// (`term`, `store`, `idx`, `pos`, `fast`, `fieldnorm`, `del`).
    pub referenced_bytes_by_extension: BTreeMap<String, u64>,
    pub referenced_bytes: u64,
    /// Segment files on disk that the last commit does not reference: the
    /// output of merges still running, or inputs of finished merges and old
    /// delete files that garbage collection has not removed yet.
    pub unreferenced_files: usize,
    pub unreferenced_bytes: u64,
    /// Distinct segment uuids among [`Self::unreferenced_files`].
    pub unreferenced_segments: usize,
    /// Every file in the directory, index-level files included.
    pub total_bytes_on_disk: u64,
}

/// Read the state of the BM25 index in `index_dir`.
///
/// Fails when the directory holds no tantivy index (no `meta.json`) or when a
/// file it lists cannot be stat'ed for a reason other than it having vanished
/// between the listing and the stat — a merge finishing under a live daemon
/// does exactly that, and is not an error of the probe.
pub fn probe_bm25_index(index_dir: &Path) -> Result<Bm25IndexStats> {
    let index = Index::open_in_dir(index_dir)
        .with_context(|| format!("no tantivy index in {}", index_dir.display()))?;
    let metas = index
        .load_metas()
        .with_context(|| format!("cannot read meta.json of {}", index_dir.display()))?;

    let mut segments = Vec::with_capacity(metas.segments.len());
    let mut referenced: HashSet<PathBuf> = HashSet::new();
    let mut segment_ids: HashSet<String> = HashSet::new();
    let mut segments_by_doc_decade: BTreeMap<u32, usize> = BTreeMap::new();
    for meta in &metas.segments {
        let max_doc = u64::from(meta.max_doc());
        let deleted_docs = u64::from(meta.num_deleted_docs());
        // `list_files` names the current delete file (`<uuid>.<opstamp>.del`)
        // as well, so an older delete file of a live segment correctly lands
        // among the unreferenced ones.
        referenced.extend(meta.list_files());
        let id = meta.id().uuid_string();
        segment_ids.insert(id.clone());
        *segments_by_doc_decade
            .entry(max_doc.checked_ilog10().unwrap_or(0))
            .or_default() += 1;
        segments.push(Bm25SegmentStats {
            id,
            max_doc,
            deleted_docs,
        });
    }

    let max_doc: u64 = segments.iter().map(|s| s.max_doc).sum();
    let deleted_docs: u64 = segments.iter().map(|s| s.deleted_docs).sum();
    let deleted_ratio = (max_doc > 0).then(|| deleted_docs as f64 / max_doc as f64);

    let mut referenced_bytes_by_extension: BTreeMap<String, u64> = BTreeMap::new();
    let mut referenced_bytes = 0u64;
    let mut unreferenced_files = 0usize;
    let mut unreferenced_bytes = 0u64;
    let mut unreferenced_segment_ids: HashSet<String> = HashSet::new();
    let mut total_bytes_on_disk = 0u64;

    let entries = std::fs::read_dir(index_dir)
        .with_context(|| format!("cannot list {}", index_dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot list {}", index_dir.display()))?;
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("cannot stat {}", entry.path().display()));
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let bytes = metadata.len();
        total_bytes_on_disk += bytes;

        let name = PathBuf::from(entry.file_name());
        let name_str = name.to_string_lossy();
        if INDEX_LEVEL_FILES.contains(&name_str.as_ref()) {
            continue;
        }
        if referenced.contains(&name) {
            referenced_bytes += bytes;
            *referenced_bytes_by_extension
                .entry(segment_file_extension(&name_str).to_string())
                .or_default() += bytes;
        } else {
            unreferenced_files += 1;
            unreferenced_bytes += bytes;
            // Segment files are `<32-hex uuid>.<ext>`; anything else (a
            // stray file someone left) still counts as unreferenced bytes,
            // just not as a segment.
            if let Some((uuid, _)) = name_str.split_once('.')
                && !segment_ids.contains(uuid)
                && uuid.len() == 32
                && uuid.bytes().all(|b| b.is_ascii_hexdigit())
            {
                unreferenced_segment_ids.insert(uuid.to_string());
            }
        }
    }

    segments.sort_by(|a, b| b.max_doc.cmp(&a.max_doc).then_with(|| a.id.cmp(&b.id)));
    segments.truncate(LARGEST_SEGMENTS_LISTED);

    Ok(Bm25IndexStats {
        path: index_dir.display().to_string(),
        segment_count: metas.segments.len(),
        max_doc,
        deleted_docs,
        live_docs: max_doc - deleted_docs,
        deleted_ratio,
        segments_by_doc_decade,
        largest_segments: segments,
        referenced_bytes_by_extension,
        referenced_bytes,
        unreferenced_files,
        unreferenced_bytes,
        unreferenced_segments: unreferenced_segment_ids.len(),
        total_bytes_on_disk,
    })
}

/// The kind of a segment file: the last dot-separated part of its name, so
/// `<uuid>.<opstamp>.del` is a `del` file like any other.
fn segment_file_extension(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::schema::{STORED, STRING, Schema, TEXT};
    use tantivy::{Term, doc};

    /// Build an index of `files` files with `chunks_per_file` documents each,
    /// committed once, then delete `deleted_files` of the files by term and
    /// commit again — the way the incremental indexer drops a changed file.
    fn index_with_deletes(dir: &Path, files: usize, chunks_per_file: usize, deleted_files: usize) {
        let mut builder = Schema::builder();
        let file_path = builder.add_text_field("file_path", STRING | STORED);
        let content = builder.add_text_field("content", TEXT | STORED);
        let index = Index::create_in_dir(dir, builder.build()).unwrap();
        // One indexing thread: one segment per commit, so the segment count
        // below is a property of the test, not of the machine.
        let mut writer = index.writer_with_num_threads(1, 15_000_000).unwrap();
        // No background merges either: the test pins what `meta.json` says,
        // and a merge finishing mid-assertion would rewrite it.
        writer.set_merge_policy(Box::new(tantivy::indexer::NoMergePolicy));
        for f in 0..files {
            for c in 0..chunks_per_file {
                writer
                    .add_document(doc!(
                        file_path => format!("src/f{f}.rs"),
                        content => format!("fn chunk_{c}() {{ body_{f}_{c} }}"),
                    ))
                    .unwrap();
            }
        }
        writer.commit().unwrap();
        for f in 0..deleted_files {
            writer.delete_term(Term::from_field_text(file_path, &format!("src/f{f}.rs")));
        }
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
    }

    #[test]
    fn probe_counts_exactly_the_written_and_deleted_documents() {
        let dir = tempfile::tempdir().unwrap();
        index_with_deletes(dir.path(), 10, 7, 3);

        let stats = probe_bm25_index(dir.path()).unwrap();

        assert_eq!(stats.segment_count, 1);
        assert_eq!(stats.max_doc, 70);
        assert_eq!(stats.deleted_docs, 21);
        assert_eq!(stats.live_docs, 49);
        assert_eq!(stats.deleted_ratio, Some(0.3));
        assert_eq!(stats.segments_by_doc_decade, BTreeMap::from([(1, 1)]));
        assert_eq!(stats.largest_segments.len(), 1);
        assert_eq!(stats.largest_segments[0].max_doc, 70);
        assert_eq!(stats.largest_segments[0].deleted_docs, 21);
        // The delete commit wrote a `.del` file the segment now references.
        assert!(stats.referenced_bytes_by_extension.contains_key("del"));
        assert!(stats.referenced_bytes_by_extension["term"] > 0);
        assert_eq!(
            stats.referenced_bytes,
            stats.referenced_bytes_by_extension.values().sum::<u64>()
        );
        assert!(stats.total_bytes_on_disk > stats.referenced_bytes);
    }

    #[test]
    fn probe_reports_an_index_without_deletes_as_all_live() {
        let dir = tempfile::tempdir().unwrap();
        index_with_deletes(dir.path(), 4, 5, 0);

        let stats = probe_bm25_index(dir.path()).unwrap();

        assert_eq!(
            (stats.max_doc, stats.deleted_docs, stats.live_docs),
            (20, 0, 20)
        );
        assert_eq!(stats.deleted_ratio, Some(0.0));
        assert!(!stats.referenced_bytes_by_extension.contains_key("del"));
    }

    #[test]
    fn probe_reports_no_ratio_for_an_empty_index() {
        let dir = tempfile::tempdir().unwrap();
        index_with_deletes(dir.path(), 0, 0, 0);

        let stats = probe_bm25_index(dir.path()).unwrap();

        assert_eq!(stats.segment_count, 0);
        assert_eq!(stats.max_doc, 0);
        assert_eq!(stats.deleted_ratio, None);
    }

    #[test]
    fn files_of_a_segment_outside_the_commit_are_unreferenced() {
        let dir = tempfile::tempdir().unwrap();
        index_with_deletes(dir.path(), 3, 2, 0);
        // What a merge in flight leaves on disk: files of a segment that
        // `meta.json` does not list yet.
        let orphan = "0123456789abcdef0123456789abcdef";
        std::fs::write(dir.path().join(format!("{orphan}.term")), [0u8; 100]).unwrap();
        std::fs::write(dir.path().join(format!("{orphan}.store")), [0u8; 50]).unwrap();

        let stats = probe_bm25_index(dir.path()).unwrap();

        assert_eq!(stats.unreferenced_files, 2);
        assert_eq!(stats.unreferenced_bytes, 150);
        assert_eq!(stats.unreferenced_segments, 1);
        assert_eq!(stats.max_doc, 6);
    }

    #[test]
    fn a_directory_without_an_index_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(probe_bm25_index(dir.path()).is_err());
    }

    #[test]
    fn delete_files_are_classified_by_their_last_extension() {
        assert_eq!(segment_file_extension("abc.1234.del"), "del");
        assert_eq!(segment_file_extension("abc.term"), "term");
    }
}
