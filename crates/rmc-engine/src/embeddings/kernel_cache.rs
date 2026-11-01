// The policy is ALWAYS compiled, but it is called only from a path under the
// `embeddings-migraphx` feature. The gates (the pure eviction function, cap parsing,
// a scene on real directories) must run in the regular suite — on a machine
// without an AMD GPU and without a system ORT nobody would run them otherwise.
#![cfg_attr(not(feature = "embeddings-migraphx"), allow(dead_code))]

//! Policy for the cache of compiled MIGraphX kernels.
//!
//! # What is stored here at all
//! MIGraphX compiles kernels FOR THE INPUT SHAPE and stores the result in `.mxr`:
//! one shape is 45–70 s of compilation and ~150–200 MB on disk. The directory is addressed
//! by model and shape (`<model>-<rows>x<seq_len>`), so programs of different
//! shapes never physically meet (see `ensure_migraphx_kernel_cache`).
//!
//! # Why a policy is needed and not just addressing
//! Addressing by shape makes growth PREDICTABLE but not BOUNDED: a directory
//! stays forever, even when its shape is no longer reachable — `max_len` was changed
//! in the profile, the batch height was raised, a second model was tried. Each
//! such step leaves ~200 MB that nobody will ever read again, and nobody
//! cleans them up. Hence a cap with eviction.
//!
//! # Why a cap and not 'remove the other shapes'
//! 'Keep only the current shape' looks more precise, but breaks exactly where
//! the cache is needed: two configurations on one machine (for example, a profile with a different
//! `max_len`, or a second server) would keep removing each other's cache in a loop,
//! paying 45–70 s of compilation on every startup. A cap does not produce such a
//! cycle: as long as the total fits, both shapes live.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Name of the 'this shape was used' marker.
///
/// The file is touched on EVERY embedder initialization, while `.mxr` is written only
/// on compilation. Without the marker, 'recency' would mean 'when it was compiled', and
/// a shape used every day would be evicted before a shape
/// compiled yesterday and abandoned.
pub(crate) const LAST_USED_MARKER: &str = ".last-used";

/// Directory of one shape in the kernel cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedShape {
    pub path: PathBuf,
    pub bytes: u64,
    pub last_used: SystemTime,
}

/// What to remove and what will remain afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvictionPlan {
    /// Directories to remove, in eviction order (least recent first).
    pub remove: Vec<PathBuf>,
    /// How many bytes will remain if the plan is executed.
    pub bytes_after: u64,
    /// The cap is unreachable even after removing EVERYTHING else — i.e. it
    /// was set below the cost of a single shape. A separate flag rather than a silent
    /// 'removed what we could': otherwise the cache would stay above the
    /// cap every time, and it would look like a broken policy.
    pub still_over_cap: bool,
}

/// Plan eviction: remove the least recently used shapes until the total fits
/// under the cap.
///
/// `keep` is NEVER removed — it is the shape the current process is bringing up
/// right now. Removing it would guarantee recompilation on the
/// very next run, i.e. a cache that wipes itself.
///
/// `cap_bytes == 0` means eviction is disabled (an explicit opt-out of the policy, not
/// 'a cap of zero').
pub(crate) fn plan_eviction(shapes: &[CachedShape], keep: &Path, cap_bytes: u64) -> EvictionPlan {
    let total: u64 = shapes.iter().map(|shape| shape.bytes).sum();
    if cap_bytes == 0 || total <= cap_bytes {
        return EvictionPlan {
            remove: Vec::new(),
            bytes_after: total,
            still_over_cap: false,
        };
    }

    let mut candidates: Vec<&CachedShape> =
        shapes.iter().filter(|shape| shape.path != keep).collect();
    // Recency is the first key, the path the second: with equal timestamps
    // (file systems with one-second granularity) the order must be
    // DETERMINISTIC, otherwise the same cache is evicted differently from
    // run to run.
    candidates.sort_by(|a, b| {
        a.last_used
            .cmp(&b.last_used)
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut remaining = total;
    let mut remove = Vec::new();
    for shape in candidates {
        if remaining <= cap_bytes {
            break;
        }
        remaining -= shape.bytes;
        remove.push(shape.path.clone());
    }

    EvictionPlan {
        remove,
        bytes_after: remaining,
        still_over_cap: remaining > cap_bytes,
    }
}

/// Total size of the files under a directory.
///
/// Read errors on individual entries are skipped on purpose: the cache directory
/// may change under us (a neighbouring process compiling its own shape), and
/// failing embedder initialization because of a race in CLEANUP is worse than being off
/// by a few megabytes.
pub(crate) fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Read the cache state: one directory per shape.
///
/// Recency is taken from the [`LAST_USED_MARKER`] marker; its absence is not an error but
/// a directory from a build that did not set the marker yet. For such a directory, recency is
/// the directory's own timestamp, i.e. 'when it was compiled' — the only
/// timestamp available there.
pub(crate) fn read_cached_shapes(root: &Path) -> Vec<CachedShape> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut shapes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let last_used = std::fs::metadata(path.join(LAST_USED_MARKER))
            .or_else(|_| std::fs::metadata(&path))
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        shapes.push(CachedShape {
            bytes: dir_size(&path),
            path,
            last_used,
        });
    }
    shapes
}

/// Mark the shape as used right now.
pub(crate) fn touch_last_used(dir: &Path) {
    // Recreating the file bumps the mtime — that is enough. Failure here is not
    // fatal: without the marker the shape simply falls under the rule 'recency = the directory's
    // time'.
    let _ = std::fs::File::create(dir.join(LAST_USED_MARKER));
}

/// Bring the cache under the cap. Returns the executed plan.
pub(crate) fn sweep(root: &Path, keep: &Path, cap_bytes: u64) -> EvictionPlan {
    let shapes = read_cached_shapes(root);
    let plan = plan_eviction(&shapes, keep, cap_bytes);
    for path in &plan.remove {
        match std::fs::remove_dir_all(path) {
            Ok(()) => tracing::info!(
                target: "embeddings::kernel_cache",
                evicted = %path.display(),
                "evicted a MIGraphX kernel cache shape"
            ),
            Err(err) => tracing::warn!(
                target: "embeddings::kernel_cache",
                path = %path.display(),
                error = %err,
                "cannot evict a MIGraphX kernel cache shape"
            ),
        }
    }
    if plan.still_over_cap {
        tracing::warn!(
            target: "embeddings::kernel_cache",
            bytes_after = plan.bytes_after,
            "MIGraphX kernel cache is over its cap even after eviction: \
             the cap is below the size of a single shape"
        );
    }
    plan
}

/// Variable that overrides the kernel cache cap.
pub(crate) const CAP_ENV: &str = "RMC_MIGRAPHX_KERNEL_CACHE_MAX_BYTES";

/// The default cap is 1 GiB, i.e. roughly five shapes of ~200 MB each.
///
/// The value is chosen from the MEASURED cost of one shape, not as a round number:
/// fewer than two shapes — and any second configuration on the machine starts paying
/// for compilation; more than five — and the cache grows faster than anyone will notice.
pub(crate) const DEFAULT_CAP_BYTES: u64 = 1 << 30;

/// Parse the cap: a bare number of bytes or a number with a `K`/`M`/`G` suffix
/// (powers of 1024). `0` disables eviction.
///
/// Garbage is an ERROR, not 'take the default': a silently ignored cap
/// reads as 'I set it', and the cache grows despite the setting.
pub(crate) fn parse_cap_bytes(raw: &str) -> Result<u64, String> {
    let raw = raw.trim();
    let (digits, multiplier) = match raw.chars().last() {
        Some('K') | Some('k') => (&raw[..raw.len() - 1], 1024),
        Some('M') | Some('m') => (&raw[..raw.len() - 1], 1024 * 1024),
        Some('G') | Some('g') => (&raw[..raw.len() - 1], 1024 * 1024 * 1024),
        _ => (raw, 1),
    };
    let value: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("`{raw}` is not a byte count (expected e.g. `2G`, `512M`, `0`)"))?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("`{raw}` overflows a byte count"))
}

/// Cap from the environment, or [`DEFAULT_CAP_BYTES`].
pub(crate) fn cap_bytes_from_env() -> Result<u64, String> {
    match std::env::var(CAP_ENV) {
        Ok(raw) if !raw.trim().is_empty() => {
            parse_cap_bytes(&raw).map_err(|err| format!("{CAP_ENV}: {err}"))
        }
        _ => Ok(DEFAULT_CAP_BYTES),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn shape(name: &str, bytes: u64, age_secs: u64) -> CachedShape {
        CachedShape {
            path: PathBuf::from("/cache").join(name),
            bytes,
            last_used: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
        }
    }

    const CAP: u64 = 500;

    #[test]
    fn under_the_cap_nothing_is_evicted() {
        let shapes = [shape("a", 200, 10), shape("b", 200, 20)];
        let plan = plan_eviction(&shapes, Path::new("/cache/a"), CAP);
        assert!(plan.remove.is_empty());
        assert_eq!(plan.bytes_after, 400);
        assert!(!plan.still_over_cap);
    }

    /// The LEAST RECENT ones are evicted, and only down to the cap — not everything.
    #[test]
    fn evicts_the_least_recently_used_until_it_fits() {
        let shapes = [
            shape("fresh", 200, 1),
            shape("stale", 200, 100),
            shape("ancient", 200, 999),
            shape("current", 200, 50),
        ];
        let plan = plan_eviction(&shapes, Path::new("/cache/current"), CAP);
        assert_eq!(
            plan.remove,
            vec![
                PathBuf::from("/cache/ancient"),
                PathBuf::from("/cache/stale"),
            ]
        );
        assert_eq!(plan.bytes_after, 400);
        assert!(!plan.still_over_cap);
    }

    /// 🔑 The current shape is not removed even when it is the least recent — otherwise the cache
    /// would wipe itself and pay compilation on every run.
    #[test]
    fn the_shape_in_use_is_never_evicted() {
        let shapes = [
            shape("current", 400, 999),
            shape("fresh_a", 200, 1),
            shape("fresh_b", 200, 2),
        ];
        let plan = plan_eviction(&shapes, Path::new("/cache/current"), CAP);
        assert!(
            !plan.remove.contains(&PathBuf::from("/cache/current")),
            "removed the shape that is being brought up right now"
        );
        assert_eq!(plan.remove.len(), 2);
        assert_eq!(plan.bytes_after, 400);
    }

    /// A cap below one shape — eviction does not stay silent but reports the outcome.
    #[test]
    fn a_cap_below_one_shape_is_reported_not_hidden() {
        let shapes = [shape("current", 900, 5), shape("other", 200, 6)];
        let plan = plan_eviction(&shapes, Path::new("/cache/current"), CAP);
        assert_eq!(plan.remove, vec![PathBuf::from("/cache/other")]);
        assert_eq!(plan.bytes_after, 900);
        assert!(
            plan.still_over_cap,
            "the cache stayed above the cap, and the policy said nothing about it"
        );
    }

    /// Zero is an explicit opt-out of the policy, not 'a cap of zero bytes'.
    #[test]
    fn zero_cap_disables_eviction() {
        let shapes = [shape("a", 10_000, 1), shape("b", 10_000, 2)];
        let plan = plan_eviction(&shapes, Path::new("/cache/a"), 0);
        assert!(plan.remove.is_empty());
        assert_eq!(plan.bytes_after, 20_000);
        assert!(!plan.still_over_cap);
    }

    /// Equal timestamps (one-second FS granularity) must not
    /// produce a different eviction order from run to run.
    #[test]
    fn equal_timestamps_evict_deterministically() {
        let shapes = [
            shape("z", 200, 42),
            shape("a", 200, 42),
            shape("m", 200, 42),
            shape("current", 200, 42),
        ];
        let first = plan_eviction(&shapes, Path::new("/cache/current"), CAP);
        let reversed: Vec<CachedShape> = shapes.iter().rev().cloned().collect();
        let second = plan_eviction(&reversed, Path::new("/cache/current"), CAP);
        assert_eq!(first, second);
        assert_eq!(
            first.remove,
            vec![PathBuf::from("/cache/a"), PathBuf::from("/cache/m")]
        );
    }

    /// End-to-end scene on real directories: the marker decides which is older, and
    /// removal actually happens.
    #[test]
    fn sweep_removes_directories_on_disk() {
        let root =
            std::env::temp_dir().join(format!("rmc-kernel-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let keep = root.join("keep");
        let stale = root.join("stale");
        for dir in [&keep, &stale] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("kernels.mxr"), vec![0u8; 300]).unwrap();
        }
        // `stale` is marked as used earlier than `keep`: both markers are set explicitly
        // so that the scene does not depend on the order in which the directories were created.
        touch_last_used(&stale);
        std::thread::sleep(Duration::from_millis(1100));
        touch_last_used(&keep);

        let plan = sweep(&root, &keep, CAP);
        assert_eq!(plan.remove, vec![stale.clone()]);
        assert!(!stale.exists(), "directory remained on disk");
        assert!(keep.exists(), "removed the shape that is in use");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cap_parser_takes_suffixes_and_rejects_garbage() {
        assert_eq!(parse_cap_bytes("0").unwrap(), 0);
        assert_eq!(parse_cap_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_cap_bytes("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_cap_bytes(" 2G ").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_cap_bytes("1k").unwrap(), 1024);
        assert!(parse_cap_bytes("").is_err());
        assert!(parse_cap_bytes("lots").is_err());
        assert!(parse_cap_bytes("2GB").is_err());
        assert!(parse_cap_bytes("-1").is_err());
        assert!(
            parse_cap_bytes("99999999999999999999G").is_err(),
            "overflow passed silently"
        );
    }
}
