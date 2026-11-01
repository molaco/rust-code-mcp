//! One process at a time runs an embedding on the GPU.
//!
//! # Why
//!
//! Two crashes in one night (2026-10-22), each with two daemons embedding on
//! one GPU at once: a successor aborted inside MIGraphX
//! (`hip.cpp:155 write_to_gpu: Assertion 'host or is_device_ptr' failed`), and
//! twenty minutes later another daemon died on `Memory access fault by GPU
//! node-1 … Page not present` in the middle of a batch while its neighbour
//! embedded a different workspace. Within a process the runs are already
//! serialised by the ONNX session; between processes nothing was. Several
//! daemons are normal here — different binaries during a rollout, different
//! configurations, other users of the same build — so "only one daemon"
//! cannot be the guarantee.
//!
//! # How
//!
//! An exclusive `flock` on one file per user, held for one run of the model
//! (one sub-batch, or one query). The kernel drops it when its holder dies, so
//! a crashed process never leaves the GPU locked. The holder writes its pid
//! into the file; a waiter names that pid in its log line, and gives up with
//! an error after [`GPU_LOCK_WAIT`] instead of hanging behind a stuck holder.
//! CPU and remote backends never take it.

use super::EmbeddingError;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Longest wait for another process's run. One run takes well under a second
/// to a few seconds; minutes mean the holder is stuck, and an error says so
/// where silence would read as a hung search.
pub const GPU_LOCK_WAIT: Duration = Duration::from_secs(300);

/// Poll interval while another process holds the lock. `flock` would block in
/// the kernel without one, but then the wait could not be bounded.
const GPU_LOCK_POLL: Duration = Duration::from_millis(20);

/// The lock file: `$XDG_RUNTIME_DIR/rust-code-mcp/gpu-embed.lock`, next to the
/// daemon sockets, or a per-user name in the temp directory without a runtime
/// directory. One per user, not per daemon key: every process on this machine
/// shares the GPU, whatever binary or configuration it runs.
pub fn gpu_lock_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir)
            .join("rust-code-mcp")
            .join("gpu-embed.lock"),
        _ => std::env::temp_dir().join(format!("rust-code-mcp-gpu-embed-{}.lock", user_tag())),
    }
}

fn user_tag() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

/// Held while this process runs the model on the GPU; dropping it unlocks.
#[derive(Debug)]
pub struct GpuLock {
    file: File,
}

impl GpuLock {
    /// Take the lock at `path`, waiting at most `wait` for another holder.
    pub fn acquire_at(path: &Path, wait: Duration) -> Result<Self, EmbeddingError> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir).map_err(|e| {
                EmbeddingError::embed_failed(format!("GPU lock directory {}: {e}", dir.display()))
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|e| {
                EmbeddingError::embed_failed(format!("GPU lock {}: {e}", path.display()))
            })?;

        let started = Instant::now();
        let mut announced = false;
        while file.try_lock_exclusive().is_err() {
            if !announced {
                // On the VRAM target, which the daemon's default filter keeps:
                // a `search` waiting here for another daemon's indexing batch
                // was otherwise invisible in its log.
                tracing::info!(
                    target: super::VRAM_LOG_TARGET,
                    "waiting for the GPU: another process is embedding ({}), lock {}",
                    holder(&mut file),
                    path.display()
                );
                announced = true;
            }
            if started.elapsed() >= wait {
                return Err(EmbeddingError::embed_failed(format!(
                    "the GPU stayed busy for {wait:?}: {} holds {}",
                    holder(&mut file),
                    path.display()
                )));
            }
            std::thread::sleep(GPU_LOCK_POLL);
        }
        if announced {
            tracing::info!(
                target: super::VRAM_LOG_TARGET,
                "got the GPU after {:.1}s",
                started.elapsed().as_secs_f64()
            );
        }
        // Who holds it, for the next waiter's log line. Best effort: the lock
        // is what matters, not the note.
        let _ = file
            .set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| write!(file, "{}", std::process::id()));
        Ok(Self { file })
    }

    /// [`GpuLock::acquire_at`] the shared path with the default wait.
    pub fn acquire() -> Result<Self, EmbeddingError> {
        Self::acquire_at(&gpu_lock_path(), GPU_LOCK_WAIT)
    }
}

impl Drop for GpuLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// "pid N" from the file, as far as it can be read.
fn holder(file: &mut File) -> String {
    let mut text = String::new();
    let read = file
        .seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_string(&mut text));
    match read.ok().and(text.trim().parse::<u32>().ok()) {
        Some(pid) => format!("pid {pid}"),
        None => "an unknown pid".to_string(),
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
    {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};

    /// Two holders on two descriptors — what two processes have — never run at
    /// once: every pair of hold intervals is disjoint.
    ///
    /// Mutation that must fail it: skip the `try_lock_exclusive` loop, and the
    /// intervals overlap.
    #[test]
    fn two_holders_never_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-embed.lock");
        let intervals = Arc::new(Mutex::new(Vec::new()));
        let start = Arc::new(Barrier::new(2));

        let workers: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let intervals = Arc::clone(&intervals);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    for _ in 0..5 {
                        let lock = GpuLock::acquire_at(&path, Duration::from_secs(30)).unwrap();
                        let from = Instant::now();
                        std::thread::sleep(Duration::from_millis(30));
                        intervals.lock().unwrap().push((from, Instant::now()));
                        drop(lock);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        let mut intervals = intervals.lock().unwrap().clone();
        assert_eq!(intervals.len(), 10, "every run took the lock");
        intervals.sort();
        for pair in intervals.windows(2) {
            assert!(
                pair[0].1 <= pair[1].0,
                "two runs held the GPU at once: {:?} and {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// A stuck holder is reported, with its pid, instead of waited on forever.
    #[test]
    fn a_stuck_holder_times_out_and_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-embed.lock");
        let _held = GpuLock::acquire_at(&path, Duration::from_secs(1)).unwrap();

        let error = GpuLock::acquire_at(&path, Duration::from_millis(100)).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(&format!("pid {}", std::process::id())),
            "{message}"
        );
    }

    /// Dropping the holder frees the lock at once — the same release the
    /// kernel performs when a holder process dies.
    #[test]
    fn a_released_lock_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-embed.lock");
        drop(GpuLock::acquire_at(&path, Duration::from_secs(1)).unwrap());
        GpuLock::acquire_at(&path, Duration::from_millis(100)).unwrap();
    }
}
