use std::collections::HashMap;
use std::path::PathBuf;

use super::{EmbeddingRecord, NodeId, OpenedSnapshot};

/// Max texts per `embed_batch_async` call. Keeps memory bounded when the
/// workspace has thousands of items.
const EMBED_CHUNK: usize = 64;

/// Per-NodeId embedding payload returned by [`ensure_embeddings_for`].
///
/// `content_hash` is exposed so callers that need to detect identical-source
/// items do not have to re-read the file and re-hash. Codemap-style callers can
/// ignore the hash.
#[derive(Debug, Clone)]
pub struct ResolvedEmbedding {
    pub vector: Vec<f32>,
    pub content_hash: [u8; 16],
}

/// Resolve embeddings for the given NodeIds, hitting the cache where possible
/// and computing-and-persisting where not.
///
/// Behaviour:
/// - Cache hit with matching `content_hash` and matching `embedder_version` is
///   reused as-is.
/// - Cache hit with mismatched hash or version is re-computed and overwritten.
/// - Cache miss is computed and inserted.
/// - Nodes missing a `file` or `span`, nodes whose file cannot be read, and
///   nodes whose `span` slices a non-UTF8 boundary / empty/whitespace region are
///   silently skipped.
///
/// Async hygiene: opens its own short read/write transactions internally.
/// Callers must not hold a `heed::RoTxn` across this call.
pub async fn ensure_embeddings_for(
    snap: &OpenedSnapshot,
    nids: &[NodeId],
    backend: &rmc_engine::embeddings::EmbeddingBackend,
) -> anyhow::Result<HashMap<NodeId, ResolvedEmbedding>> {
    use sha2::{Digest, Sha256};

    let mut out: HashMap<NodeId, ResolvedEmbedding> = HashMap::new();
    if nids.is_empty() {
        return Ok(out);
    }

    // The workspace root from the snapshot manifest is the base for the
    // workspace-relative `Node.file` strings.
    let ws_root = PathBuf::from(&snap.manifest.workspace_root);

    // Phase A: classify nids using one short read txn.
    struct Pending {
        nid: NodeId,
        content_hash: [u8; 16],
        source: String,
    }
    let mut pending: Vec<Pending> = Vec::new();
    let mut file_cache: HashMap<String, String> = HashMap::new();

    // The cache classifier and the embedder are always in lockstep because
    // they both derive from the same backend.
    let active_version = backend.identity();

    {
        let rtxn = snap.env.read_txn()?;
        let mut seen: std::collections::HashSet<NodeId> =
            std::collections::HashSet::with_capacity(nids.len());
        for &nid in nids {
            if !seen.insert(nid) {
                continue;
            }
            let Some(node) = snap.node_by_id(&rtxn, nid)? else {
                continue;
            };
            let Some(file_rel) = node.file.as_deref() else {
                continue;
            };
            let Some(span) = node.span else {
                continue;
            };

            let abs_path = ws_root.join(file_rel);
            let abs_key = abs_path.to_string_lossy().to_string();
            if !file_cache.contains_key(&abs_key) {
                match std::fs::read_to_string(&abs_path) {
                    Ok(s) => {
                        file_cache.insert(abs_key.clone(), s);
                    }
                    Err(_) => continue,
                }
            }
            let content = file_cache.get(&abs_key).expect("inserted above");
            let (start, end) = (span.0 as usize, span.1 as usize);
            let Some(slice) = content.get(start..end) else {
                continue;
            };
            let trimmed = slice.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut hasher = Sha256::new();
            hasher.update(trimmed.as_bytes());
            let full = hasher.finalize();
            let mut content_hash = [0u8; 16];
            content_hash.copy_from_slice(&full[..16]);

            match snap.dbs.embeddings_by_target.get(&rtxn, nid.as_bytes())? {
                Some(rec)
                    if rec.content_hash == content_hash
                        && rec.embedder_version == active_version =>
                {
                    out.insert(
                        nid,
                        ResolvedEmbedding {
                            vector: rec.vector,
                            content_hash,
                        },
                    );
                }
                _ => {
                    pending.push(Pending {
                        nid,
                        content_hash,
                        source: trimmed.to_string(),
                    });
                }
            }
        }
    }

    if pending.is_empty() {
        return Ok(out);
    }

    let embedder = rmc_engine::embeddings::EmbeddingGenerator::with_backend(backend.clone())
        .map_err(|e| anyhow::anyhow!("EmbeddingGenerator init: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut new_vectors: Vec<Vec<f32>> = Vec::with_capacity(pending.len());
    for chunk_start in (0..pending.len()).step_by(EMBED_CHUNK) {
        let chunk_end = (chunk_start + EMBED_CHUNK).min(pending.len());
        let texts: Vec<String> = pending[chunk_start..chunk_end]
            .iter()
            .map(|p| p.source.clone())
            .collect();
        let vectors = embedder
            .embed_documents(texts)
            .await
            .map_err(|e| anyhow::anyhow!("embed_documents: {e}"))?;
        new_vectors.extend(vectors);
    }

    {
        let mut wtxn = snap.env.write_txn()?;
        for (p, vector) in pending.iter().zip(new_vectors.iter()) {
            let rec = EmbeddingRecord {
                content_hash: p.content_hash,
                vector: vector.clone(),
                embedder_version: active_version.clone(),
                generated_at_unix: now,
            };
            snap.dbs
                .embeddings_by_target
                .put(&mut wtxn, p.nid.as_bytes(), &rec)?;
        }
        wtxn.commit()?;
    }

    for (p, vector) in pending.into_iter().zip(new_vectors.into_iter()) {
        out.insert(
            p.nid,
            ResolvedEmbedding {
                vector,
                content_hash: p.content_hash,
            },
        );
    }

    Ok(out)
}
