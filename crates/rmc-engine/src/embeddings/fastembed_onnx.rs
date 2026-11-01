//! Text embedding backend backed by fastembed's ONNX path — CPU or AMD GPU.
//!
//! The two planes differ in EXACTLY one thing: the list of execution providers that
//! goes into `TextInitOptions::with_execution_providers`. fastembed itself knows
//! nothing about GPU EPs and should not have to — it accepts the EPs from outside.

use crate::embeddings::backend::{EmbeddingBackend, EmbeddingRuntime};
use crate::embeddings::batching::FixedInputShape;
use crate::embeddings::ep_census::ProviderCensus;
use crate::embeddings::profile::FastembedOnnxModel;
use crate::embeddings::{Embedding, EmbeddingError};
use crate::embeddings::gpu_broker::Admission;
use crate::embeddings::vram_ledger::{read_vram, EmbeddingOwner, VramTicket};
use fastembed::{EmbeddingModel, FixedBatchShape, TextEmbedding, TextInitOptions};
use std::sync::Mutex;

pub(super) struct FastembedOnnxEmbedder {
    inner: Mutex<Session>,
    backend: EmbeddingBackend,
    dim: usize,
}

/// The ORT session and, on a GPU runtime, its entry in the VRAM ledger.
///
/// Together under one mutex because the ledger's "first run" is a property of
/// the session's runs, which that mutex already serialises.
///
/// 🚨 Field order is load-bearing: `model` drops before `ticket`, so the
/// ticket's drop sees the memory the session gave back. The reading before
/// it is taken in `Session::drop`, which runs before either field drops.
struct Session {
    model: TextEmbedding,
    ticket: Option<VramTicket>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.as_mut() {
            ticket.release_starting();
        }
    }
}

impl FastembedOnnxEmbedder {
    /// A CPU session. GPU backends are refused: their sessions are made only
    /// by `gpu_broker`, through [`Self::new_gpu`].
    pub(super) fn new_cpu(
        backend: &EmbeddingBackend,
        owner: EmbeddingOwner,
    ) -> Result<Self, EmbeddingError> {
        if backend.is_fastembed_onnx_gpu() {
            return Err(EmbeddingError::model_init(format!(
                "profile `{}` runs on the GPU; its session is created by the GPU broker only",
                backend.profile.name()
            )));
        }
        Self::new_inner(backend, owner, None)
    }

    /// A GPU session. The [`Admission`] is the broker's budget check, and the
    /// only way to get one; it is what keeps a second session per backend
    /// from being built behind the broker's back.
    pub(super) fn new_gpu(
        backend: &EmbeddingBackend,
        owner: EmbeddingOwner,
        _admitted: &Admission,
    ) -> Result<Self, EmbeddingError> {
        Self::new_inner(backend, owner, None)
    }

    /// The same initialization path, but with ORT profiling enabled.
    ///
    /// Profiling CANNOT be enabled after the session is built, hence a separate
    /// constructor: the same `backend`, the same EP list, the same shape — otherwise
    /// the profile would describe a session other than the one running in production, and the oracle
    /// would gate its own copy of the code.
    ///
    /// `prefix` is the file name prefix; ORT appends a timestamp to it,
    /// the actual path is returned by [`Self::end_profiling`].
    fn new_profiled(
        backend: &EmbeddingBackend,
        prefix: &std::path::Path,
    ) -> Result<Self, EmbeddingError> {
        Self::new_inner(backend, EmbeddingOwner::ProviderCensus, Some(prefix))
    }

    /// Close the ORT profile and return the path of the written file.
    fn end_profiling(&self) -> Result<std::path::PathBuf, EmbeddingError> {
        let mut session = self.inner.lock().unwrap();
        session
            .model
            .end_profiling()
            .map(std::path::PathBuf::from)
            .map_err(|e| EmbeddingError::model_init(e.to_string()))
    }

    fn new_inner(
        backend: &EmbeddingBackend,
        owner: EmbeddingOwner,
        profiling_prefix: Option<&std::path::Path>,
    ) -> Result<Self, EmbeddingError> {
        if !backend.is_fastembed_onnx() {
            return Err(EmbeddingError::model_init(format!(
                "embedding profile `{}` is not a fastembed ONNX profile",
                backend.profile.name()
            )));
        }
        let model = backend.require_fastembed_onnx_model()?;
        let on_gpu = backend.is_fastembed_onnx_gpu();

        tracing::info!(
            target: "embeddings::fastembed_onnx",
            profile = backend.profile.name(),
            model = model.display_name(),
            max_len = backend.max_len,
            on_gpu,
            "loading fastembed ONNX model"
        );

        // The input shape is computed BEFORE the session is created: the kernel cache
        // directory depends on it and must be set before the EP takes control.
        //
        // The value is declared by the BACKEND — the indexer reads the same value when it cuts
        // inputs into batches. The refusal here is not hypothetical: it fires if
        // someone adds a GPU runtime and forgets the second half of the pair
        // 'runtime ⇄ shape', and then we would silently pay kernel compilation for every
        // random input shape.
        let shape = backend.fixed_input_shape().map(to_fastembed_shape);
        if on_gpu && shape.is_none() {
            return Err(EmbeddingError::model_init(format!(
                "profile `{}` runs on a GPU execution provider but declares no fixed input shape",
                backend.profile.name()
            )));
        }

        let mut options = TextInitOptions::new(to_fastembed_model(model))
            .with_max_length(backend.max_len)
            .with_show_download_progress(false)
            .with_cache_dir(model_cache_dir()?);
        if let (true, Some(shape)) = (on_gpu, shape) {
            options = options.with_execution_providers(gpu_execution_providers(
                backend.runtime,
                model,
                shape,
            )?);
        }
        if let Some(prefix) = profiling_prefix {
            options = options.with_profiling(prefix.to_path_buf());
        }
        // Read before the session exists, so its creation is charged to it.
        let before_creation = on_gpu.then(read_vram);
        let mut inner = TextEmbedding::try_new(options)
            .map_err(|e| EmbeddingError::model_init(e.to_string()))?;
        if let (true, Some(shape)) = (on_gpu, shape) {
            inner = inner
                .with_fixed_batch_shape(shape)
                .map_err(|e| EmbeddingError::model_init(e.to_string()))?;
            tracing::info!(
                target: "embeddings::fastembed_onnx",
                rows = shape.rows,
                seq_len = shape.seq_len,
                runtime = ?backend.runtime,
                "fixed GPU input shape"
            );
        }

        let ticket = before_creation
            .map(|before| VramTicket::open(owner, backend.profile.name(), before));

        Ok(Self {
            inner: Mutex::new(Session {
                model: inner,
                ticket,
            }),
            backend: backend.clone(),
            dim: backend.dim(),
        })
    }

    pub(super) fn dim(&self) -> usize {
        self.dim
    }

    pub(super) fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
        let mut session = self.inner.lock().unwrap();
        // The first run is where MIGraphX puts the program on the GPU; see
        // `vram_ledger` for why it is charged separately from creation.
        let before_first_run = session
            .ticket
            .as_ref()
            .filter(|t| t.first_run_pending())
            .map(|_| read_vram());
        let result = session
            .model
            .embed(texts, None)
            .map_err(|e| EmbeddingError::embed_failed(e.to_string()));
        if let (Some(before), Some(ticket)) = (before_first_run, session.ticket.as_mut()) {
            ticket.first_run_finished(before);
        }
        result
    }

    /// One run on a throw-away text, so that the session's first run — where
    /// MIGraphX puts the program on the GPU — happens now, inside the
    /// broker's admitted window, and not in somebody's query later.
    ///
    /// One text is enough: the session has a fixed input shape, so a single
    /// row is padded to the same `rows × seq_len` every real batch runs as.
    pub(super) fn warm_up(&self) -> Result<(), EmbeddingError> {
        self.embed_documents(&["fn warm_up() {}"]).map(|_| ())
    }

    pub(super) fn embed_queries(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|text| self.backend.format_query(text))
            .collect();
        let refs: Vec<&str> = prefixed.iter().map(String::as_str).collect();
        self.embed_documents(&refs)
    }
}

/// Corpus for the EP census probe.
///
/// Texts of DIFFERENT lengths on purpose: on them the floating and the constant shape give
/// different padding, so the probe takes the same path through the model as a real
/// run rather than a degenerate one.
const CENSUS_PROBE_CORPUS: [&str; 4] = [
    "fn main() {}",
    "pub struct WorkspaceLockRegistry { global: Arc<Mutex<()>> }",
    "impl Iterator for Chunks { type Item = CodeChunk; fn next(&mut self) -> Option<Self::Item> { self.inner.next() } }",
    "async fn index_codebase(params: IndexCodebaseParams, sync: Option<&Arc<SyncManager>>) -> Result<CallToolResult, McpError>",
];

/// One profiled embedder run + a census of 'nodes per provider'.
///
/// # Why this is in production and not only in tests
/// A census test gates the MACHINE it was run on, where someone remembered
/// to run it. The question 'did the graph really go to the GPU on this machine' is asked of
/// a live server, where neither the build feature, nor the system ORT, nor the driver version is
/// what the test had. So the same census is available as a runtime probe.
///
/// # Cost
/// The probe brings up a SEPARATE session (profiling cannot be enabled after the
/// session is built), i.e. it loads the model again, and on a cold kernel cache
/// it also pays the MIGraphX compilation (45–70 s). That is why it is called only via
/// an explicit knob, not on every startup.
///
/// The ORT profile is a temporary file: it is needed only while it is being parsed, and there is
/// no reason to leave hundreds of megabytes of JSON on disk after every probe. The directory is removed
/// even when parsing fails.
pub(super) fn probe_provider_census(
    backend: &EmbeddingBackend,
) -> Result<ProviderCensus, EmbeddingError> {
    // The directory name is per pid: two servers on one machine must not share
    // a profile directory, otherwise one's census would see the other's files.
    let dir = std::env::temp_dir().join(format!("rmc-ep-census-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| {
        EmbeddingError::model_init(format!(
            "cannot create ORT profile directory at {}: {e}",
            dir.display()
        ))
    })?;

    let census = (|| {
        // The census session is profiled from its creation, so it cannot be
        // the broker's shared one; it is a second session for the duration of
        // the probe, and passes the same checks, under the same lock, held
        // for the whole probe: its first run allocates like any session's.
        let _gpu = backend
            .is_fastembed_onnx_gpu()
            .then(crate::embeddings::gpu_lock::GpuLock::acquire)
            .transpose()?;
        if backend.is_fastembed_onnx_gpu() {
            crate::embeddings::gpu_broker::admit(backend)?;
        }
        let embedder = FastembedOnnxEmbedder::new_profiled(backend, &dir.join("census"))?;
        // The census must be based on the session's WORK: before the first run
        // the profile is empty, and 'zero nodes on GPU' would mean 'did not look'.
        embedder.embed_documents(&CENSUS_PROBE_CORPUS)?;
        ProviderCensus::from_profile_file(&embedder.end_profiling()?)
    })();

    let _ = std::fs::remove_dir_all(&dir);
    census
}

/// Our shape → the vendor's shape.
///
/// A separate translator function so that fastembed's `FixedBatchShape` does not
/// spread beyond this module: the indexer needs the shape, but not a dependency on the
/// vendor.
fn to_fastembed_shape(shape: FixedInputShape) -> FixedBatchShape {
    FixedBatchShape {
        rows: shape.rows.0,
        seq_len: shape.seq_len,
    }
}

fn to_fastembed_model(model: FastembedOnnxModel) -> EmbeddingModel {
    match model {
        FastembedOnnxModel::BgeSmallEnV15Q => EmbeddingModel::BGESmallENV15Q,
        FastembedOnnxModel::BgeSmallEnV15 => EmbeddingModel::BGESmallENV15,
    }
}

/// EP dispatch type. It is the same type in both forms — `fastembed`
/// re-exports it from `ort`; the alias is needed because the `ort` crate itself
/// appears in the dependencies only together with the GPU features.
#[cfg(any(feature = "embeddings-migraphx", feature = "embeddings-directml"))]
type EpDispatch = ort::execution_providers::ExecutionProviderDispatch;
#[cfg(not(any(feature = "embeddings-migraphx", feature = "embeddings-directml")))]
type EpDispatch = fastembed::ExecutionProviderDispatch;

/// EPs for the GPU runtime — one per OS, and this is not duplication.
///
/// MIGraphX (Linux) and DirectML (Windows) are not interchangeable: MIGraphX does not
/// exist on Windows at all (not the whole ROCm stack has been ported to Windows), and
/// DirectML is a Windows API on top of DX12 and, symmetrically, does not exist on Linux
/// either. So the runtime selects a PROFILE rather than auto-detecting: the choice of
/// EP here is a choice of index (the runtime is part of `EmbeddingIdentity`).
fn gpu_execution_providers(
    runtime: EmbeddingRuntime,
    model: FastembedOnnxModel,
    shape: FixedBatchShape,
) -> Result<Vec<EpDispatch>, EmbeddingError> {
    match runtime {
        EmbeddingRuntime::LocalFastembedOnnxMigraphx => migraphx_execution_providers(model, shape),
        EmbeddingRuntime::LocalFastembedOnnxDirectml => directml_execution_providers(),
        other => Err(EmbeddingError::model_init(format!(
            "runtime {other:?} is not a fastembed ONNX GPU runtime"
        ))),
    }
}

/// EP list for AMD GPU on LINUX.
///
/// # Why only MIGraphX here
/// The ROCm EP is deprecated in ONNX Runtime, and the shipped builds physically
/// do not contain it: next to `libonnxruntime.so` there is only
/// `libonnxruntime_providers_migraphx.so`. Asking for the ROCm EP means getting
/// a silent fallback to CPU.
///
/// # Oracle against a silent fallback
/// `error_on_failure()` turns 'the EP did not come up' from silent degradation into
/// an initialization error. Without it the session is created, numbers get computed, and 'the GPU
/// works' becomes a false conclusion — exactly the class of error this code
/// must avoid.
#[cfg(feature = "embeddings-migraphx")]
fn migraphx_execution_providers(
    model: FastembedOnnxModel,
    shape: FixedBatchShape,
) -> Result<Vec<EpDispatch>, EmbeddingError> {
    ensure_migraphx_kernel_cache(model, shape)?;
    Ok(vec![
        ort::ep::migraphx::MIGraphX::default()
            .build()
            .error_on_failure(),
    ])
}

#[cfg(not(feature = "embeddings-migraphx"))]
fn migraphx_execution_providers(
    _model: FastembedOnnxModel,
    _shape: FixedBatchShape,
) -> Result<Vec<EpDispatch>, EmbeddingError> {
    Err(EmbeddingError::model_init(
        "rmc-engine was built without the `embeddings-migraphx` feature; \
         rebuild with --features migraphx to use GPU embedding profiles",
    ))
}

/// EP list for GPU on WINDOWS — DirectML.
///
/// # Why the input shape brings no arguments here
/// Unlike MIGraphX, DirectML does not compile kernels into files and needs
/// neither a cache directory nor an environment variable: it has neither a cold
/// start of 45–70 s nor `.mxr` files of 145–200 MB per shape. The shape is still
/// declared (see `fixed_input_shape`), but the EP does not need to know about it —
/// the session gets it through fastembed's fixed batch.
///
/// # What SILENTLY breaks here without `fastembed/directml`
/// The DirectML EP does not survive memory patterns and parallel execution; they are turned off
/// by the vendored fastembed, and it recognizes DirectML in the provider list only
/// under its own feature. So the feature is hard-enabled in `embeddings-directml` —
/// building 'only ort/directml' is technically possible, and it would produce a session
/// that fails somewhere other than in this file.
#[cfg(feature = "embeddings-directml")]
fn directml_execution_providers() -> Result<Vec<EpDispatch>, EmbeddingError> {
    Ok(vec![
        ort::ep::directml::DirectML::default()
            .build()
            .error_on_failure(),
    ])
}

#[cfg(not(feature = "embeddings-directml"))]
fn directml_execution_providers() -> Result<Vec<EpDispatch>, EmbeddingError> {
    Err(EmbeddingError::model_init(
        "rmc-engine was built without the `embeddings-directml` feature; \
         rebuild with --features directml to use the `local-dml-bge` profile",
    ))
}

/// Directory of the cache of compiled MIGraphX kernels (`.mxr`).
///
/// # Why this is not tuning but a precondition for working at all
/// ONNX Runtime 1.28 writes the compiled program EVEN when saving was not
/// requested, and takes the directory ONLY from `ORT_MIGRAPHX_MODEL_CACHE_PATH`. If
/// it is unset, the path is built from an empty string and the write fails — and what fails is not
/// initialization but the first `run`, already at runtime. That is why the variable
/// is set here, before the session is created, rather than left to the
/// caller's discretion.
///
/// Neighbouring knobs do NOT affect this path, checked one by one: neither the provider option
/// `migraphx_save_model_path` (ort passes it in the deprecated struct
/// `OrtMIGraphXProviderOptions`, and ORT 1.28 ignores it), nor the variable
/// `ORT_MIGRAPHX_CACHE_PATH`, which strings shows in the same library.
///
/// # Why the directory is addressed by input shape
/// The `.mxr` name includes a graph hash but does NOT distinguish shapes: when loading a file from
/// a DIFFERENT shape, the first `run` after startup returns a result for the CACHED shape rather than
/// for the actual input — silently, with no error (reproduced: input 16×512, output
/// `[32, 512, 384]`). So the directory is named by model and shape: programs
/// of different shapes never physically meet, and an old cache of mixed shapes is not
/// picked up. Compiling one shape costs 45–70 s and ~145–200 MB.
///
/// An explicit `ORT_MIGRAPHX_MODEL_CACHE_PATH` is respected but treated as a ROOT:
/// the shape subdirectory is appended to it as well — the invariant 'one directory = one
/// shape' must not depend on whether someone set the variable.
/// Where downloaded model weights live.
///
/// fastembed's own default is `.fastembed_cache` **relative to the process's
/// working directory** (`vendor/fastembed/src/common.rs`), which put a copy of
/// the weights in whatever tree a session happened to start in — four of them,
/// 128–279 MB each, were on this machine before this was fixed. One daemon now
/// serves every working directory, so that default would have made the location
/// of the weights depend on which session woke the daemon first. Same XDG root
/// as the MIGraphX kernel cache below, so both caches are found in one place.
fn model_cache_dir() -> Result<std::path::PathBuf, EmbeddingError> {
    directories::ProjectDirs::from("", "", "rust-code-mcp")
        .map(|d| d.cache_dir().join("models"))
        .ok_or_else(|| {
            EmbeddingError::model_init("cannot resolve a cache directory for model weights")
        })
}

#[cfg(feature = "embeddings-migraphx")]
fn ensure_migraphx_kernel_cache(
    model: FastembedOnnxModel,
    shape: FixedBatchShape,
) -> Result<std::path::PathBuf, EmbeddingError> {
    const CACHE_ENV: &str = "ORT_MIGRAPHX_MODEL_CACHE_PATH";

    let shape_dir = format!("{}-{}x{}", model.display_name(), shape.rows, shape.seq_len);

    // The root is read from the environment ONCE per process and memoized: below we
    // ourselves write the shape subdirectory path into the same variable, and re-reading
    // would take our own answer for the root — directories would nest inside each
    // other with a second embedder in the same process.
    static CACHE_ROOT: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    let root = CACHE_ROOT
        .get_or_init(|| {
            std::env::var_os(CACHE_ENV)
                .filter(|v| !v.is_empty())
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    directories::ProjectDirs::from("", "", "rust-code-mcp")
                        .map(|d| d.cache_dir().join("migraphx"))
                })
        })
        .clone()
        .ok_or_else(|| {
            EmbeddingError::model_init("cannot resolve a cache directory for MIGraphX kernels")
        })?;
    let dir = root.join(shape_dir);
    std::fs::create_dir_all(&dir).map_err(|e| {
        EmbeddingError::model_init(format!(
            "cannot create MIGraphX kernel cache at {}: {e}",
            dir.display()
        ))
    })?;

    // The 'this shape is in use' mark is set BEFORE the sweep: otherwise the shape we
    // are bringing up right now would be queued for eviction based on stale recency.
    // It would not have been removed anyway (`keep`), but its recency would remain
    // a lie for the next run.
    crate::embeddings::kernel_cache::touch_last_used(&dir);
    let cap_bytes = crate::embeddings::kernel_cache::cap_bytes_from_env()
        .map_err(EmbeddingError::model_init)?;
    let plan = crate::embeddings::kernel_cache::sweep(&root, &dir, cap_bytes);

    tracing::info!(
        target: "embeddings::fastembed_onnx",
        cache = %dir.display(),
        cap_bytes,
        cache_bytes = plan.bytes_after,
        evicted_shapes = plan.remove.len(),
        "MIGraphX kernel cache"
    );
    // SAFETY: called on the embedder initialization path, before the ORT session is created
    // and before any background threads exist that could read the environment.
    unsafe { std::env::set_var(CACHE_ENV, &dir) };
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texts of different lengths: the batch must contain both short and
    /// long ones — on those `BatchLongest` and the constant shape produce DIFFERENT padding,
    /// and hence a different path through the model.
    fn corpus() -> Vec<String> {
        vec![
            "fn main() {}".to_string(),
            "pub struct ChunkId(pub u64);".to_string(),
            "async fn embed_documents(&self, texts: Vec<String>) -> Result<Vec<Embedding>> { \
             let refs = texts.iter().map(String::as_str).collect::<Vec<_>>(); \
             self.inner.embed(&refs, None) }"
                .to_string(),
            "// comment".to_string(),
            "impl Display for EmbeddingError { fn fmt(&self, f: &mut Formatter) -> fmt::Result }"
                .to_string(),
        ]
    }

    /// The tests below pull THE SAME model file through hf-hub, which takes
    /// a file lock on the blob: two parallel tests fight over it and one
    /// fails with 'Lock acquisition failed'. Model loading is therefore
    /// serialized — this is about the HF cache, not about fastembed thread-safety.
    fn model_guard() -> std::sync::MutexGuard<'static, ()> {
        static MODEL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>()
    }

    /// Oracle for the constant input shape: it changes neither the NUMBERS nor the
    /// COUNT of embeddings.
    ///
    /// Both properties are checked because they break in different ways:
    /// - count — if padding rows leaked out (were not cut off by
    ///   `real_rows`); caught on an input whose length is NOT a multiple of the batch height;
    /// - numbers — if padding to the fixed length started affecting the result
    ///   (e.g. the attention mask stopped masking out the tail). The reference here is
    ///   the same fastembed without a fixed shape, so the comparison is fair:
    ///   exactly one thing changes.
    ///
    /// The test runs on CPU and so needs no GPU, but it needs a downloaded
    /// model and a full forward pass — hence `#[ignore]`; run it explicitly:
    /// `cargo test -p rmc-engine --features embeddings fixed_batch_shape -- --ignored`
    #[test]
    #[ignore = "downloads the model from HF and runs forward on CPU"]
    fn fixed_batch_shape_preserves_embeddings() {
        let texts = corpus();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();

        let _guard = model_guard();
        let options = || {
            TextInitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_max_length(512)
                .with_show_download_progress(false)
        };

        let mut baseline = TextEmbedding::try_new(options()).unwrap();
        let expected = baseline.embed(&refs, None).unwrap();

        // rows=4 with 5 texts: two batches, the second one padded with three rows.
        // This non-multiple is exactly what makes the test a gate on cutting off the tail.
        let shape = FixedBatchShape {
            rows: 4,
            seq_len: 512,
        };
        let mut fixed = TextEmbedding::try_new(options())
            .unwrap()
            .with_fixed_batch_shape(shape)
            .unwrap();
        assert_eq!(fixed.fixed_batch_shape(), Some(shape));
        let actual = fixed.embed(&refs, None).unwrap();

        assert_eq!(
            actual.len(),
            texts.len(),
            "padding rows leaked out: more embeddings than texts"
        );
        for (idx, (want, got)) in expected.iter().zip(&actual).enumerate() {
            let sim = cosine(want, got);
            assert!(
                sim > 0.999,
                "text #{idx}: the constant shape changed the embedding (cosine {sim})"
            );
        }
    }

    /// Refusals that must be refusals, not a silent change of shape.
    #[test]
    #[ignore = "downloads the model from HF"]
    fn fixed_batch_shape_rejects_impossible_shapes() {
        let _guard = model_guard();
        let options = || {
            TextInitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_max_length(512)
                .with_show_download_progress(false)
        };

        // Sequence length beyond the truncation limit: the promised shape cannot
        // be obtained — the tokenizer would truncate anyway.
        let err = TextEmbedding::try_new(options())
            .unwrap()
            .with_fixed_batch_shape(FixedBatchShape {
                rows: 32,
                seq_len: 1024,
            })
            .err()
            .expect("seq_len beyond the truncation limit must be refused");
        assert!(err.to_string().contains("truncation limit"), "{err}");

        let err = TextEmbedding::try_new(options())
            .unwrap()
            .with_fixed_batch_shape(FixedBatchShape {
                rows: 0,
                seq_len: 512,
            })
            .err()
            .expect("a zero batch height must be refused");
        assert!(err.to_string().contains("non-zero"), "{err}");
    }

    /// Oracle for the GPU path: the graph is ACTUALLY computed on MIGraphX, not on CPU.
    ///
    /// # What exactly it catches
    /// `error_on_failure()` on the EP only covers 'the provider did not come up'.
    /// The class 'the EP came up but took zero nodes' passes straight through it: the session
    /// is alive, embeddings get computed, only the speed differs — i.e. before this
    /// test the degradation was visible only by eye and only in a benchmark.
    ///
    /// # Why the assertion is about CPU nodes and not about a ratio
    /// MIGraphX does not 'take nodes one by one': it cuts out a subgraph, compiles it
    /// and substitutes ONE fused node. Measured on this scene: a healthy
    /// GPU path gives `MIGraphXExecutionProvider=1` and zero CPU nodes, whereas
    /// the same corpus on the CPU profile gives 365 nodes (see the positive control below).
    /// So a ratio does not work as a metric here: a single unit 'weighs' the whole
    /// graph. We assert two properties: the fused node EXISTS, and the CPU has not accumulated
    /// a noticeable tail — i.e. the subgraph was not nibbled away piece by piece.
    ///
    /// The slack of 32 nodes is not a measured value but a margin: shape operators
    /// (Shape/Reshape/Cast) may in principle stay outside the subgraph, as
    /// seen on the python side with the ROCm EP (4158 nodes on GPU and 48 on CPU there — but
    /// the ROCm EP does not fuse, and its node picture is different). It is an order of magnitude
    /// below 365, so a 'graph went back to CPU' regression is caught with margin to spare.
    ///
    /// Requires a GPU, a system ORT with MIGraphX and a downloaded model, hence
    /// `#[ignore]`; the first run on a cold kernel cache pays ~a minute of
    /// compilation. Run:
    /// `cargo test -p rmc-engine --features embeddings-migraphx migraphx_ep -- --ignored --nocapture`
    #[cfg(feature = "embeddings-migraphx")]
    #[test]
    #[ignore = "needs an AMD GPU, ORT with MIGraphX and a downloaded model"]
    fn migraphx_ep_actually_runs_the_graph() {
        use crate::embeddings::ep_census::{CPU_EP, MIGRAPHX_EP};

        let _guard = model_guard();
        let backend = EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();

        // The gate calls EXACTLY the same probe the server calls via its knob: otherwise it
        // would be checking its own copy of the path, and the runtime diagnostics would remain
        // ungated.
        let census = probe_provider_census(&backend).unwrap();
        eprintln!("nodes per provider: {census}");

        assert!(
            census.nodes_on(MIGRAPHX_EP) > 0,
            "not a single node went to MIGraphX — silent fallback to CPU: {census}"
        );
        const CPU_TAIL_SLACK: usize = 32;
        assert!(
            census.nodes_on(CPU_EP) <= CPU_TAIL_SLACK,
            "{} nodes left on CPU (slack {CPU_TAIL_SLACK}) — the subgraph did not move to the GPU entirely: {census}",
            census.nodes_on(CPU_EP),
        );
    }

    /// One full GPU batch: 32 texts fill the fixed 32×512 shape of `local-gpu-bge`.
    #[cfg(feature = "embeddings-migraphx")]
    fn full_gpu_batch() -> Vec<String> {
        (0..32)
            .map(|i| format!("fn probe_{i}(x: u64) -> u64 {{ x.wrapping_mul({i}) }} ").repeat(40))
            .collect()
    }

    #[cfg(feature = "embeddings-migraphx")]
    fn owner_totals(owner: EmbeddingOwner) -> crate::embeddings::OwnerReport {
        crate::embeddings::gpu_ledger_report()
            .owners
            .into_iter()
            .find(|o| o.owner == owner)
            .unwrap()
    }

    /// Oracle for the VRAM ledger: what it charges a session is what the card
    /// lost, ±5 %.
    ///
    /// The ledger weighs sessions with the process meter (DRM fdinfo); the
    /// reference is the card meter (sysfs), read by this test around the
    /// whole create-and-first-run window, outside the ledger's code. The
    /// window is held under the cross-process GPU lock, so no other daemon's
    /// run lands inside it. Also checked: the first run is charged at all —
    /// MIGraphX puts its program on the GPU there, not at creation, and a
    /// ledger that weighed only creation would miss most of the session.
    ///
    /// `cargo test --release -p rmc-engine --features embeddings-migraphx vram_ledger_charge -- --ignored --nocapture`
    #[cfg(feature = "embeddings-migraphx")]
    #[test]
    #[ignore = "needs an AMD GPU, ORT with MIGraphX and a downloaded model"]
    fn vram_ledger_charge_matches_the_card() {
        let _guard = model_guard();
        let backend = EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        let texts = full_gpu_batch();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();

        let _gpu = crate::embeddings::gpu_lock::GpuLock::acquire().unwrap();
        let card_before = read_vram().card_used_bytes.expect("no amdgpu sysfs meter");
        let embedder = FastembedOnnxEmbedder::new_gpu(&backend, EmbeddingOwner::Tool, &Admission::for_probe()).unwrap();
        embedder.embed_documents(&refs).unwrap();
        let card_after = read_vram().card_used_bytes.unwrap();

        let report = crate::embeddings::gpu_ledger_report();
        let session = report
            .live_sessions
            .iter()
            .filter(|s| s.owner == EmbeddingOwner::Tool)
            .max_by_key(|s| s.id)
            .unwrap();
        eprintln!("session: {session:?}");
        eprintln!("card delta: {}", card_after as i64 - card_before as i64);

        let first_run = session.at_first_run.expect("first run not recorded");
        assert!(
            first_run.process_bytes.unwrap() > 0,
            "first run charged nothing: {session:?}"
        );
        let charged = session.charge.process_bytes.unwrap() as f64;
        let card = (card_after as i64 - card_before as i64) as f64;
        assert!(card > 0.0, "the card did not move: {card}");
        let error = (charged - card).abs() / card;
        assert!(
            error <= 0.05,
            "ledger charged {charged} bytes, card moved {card} ({:.1} %)",
            error * 100.0
        );
    }

    /// Probe for `leak-repro`: does dropping a session give its VRAM back?
    ///
    /// N × (create a session, one full 32×512 batch, drop), printing both
    /// meters and the ledger's charge/returned after every cycle. Answers the
    /// question rather than asserting an answer: how much each cycle keeps,
    /// and whether it is the drop that leaks.
    ///
    /// The card is shared with live daemons, so the loop stops before the
    /// card is pushed into an allocation failure: below
    /// `RMC_LEAK_PROBE_MIN_FREE_MB` free (default 3072) it reports and ends.
    /// `RMC_LEAK_PROBE_CYCLES` (default 20).
    ///
    /// `cargo test --release -p rmc-engine --features embeddings-migraphx session_drop_leak_probe -- --ignored --nocapture`
    #[cfg(feature = "embeddings-migraphx")]
    #[test]
    #[ignore = "needs an AMD GPU, ORT with MIGraphX and a downloaded model; a probe, not a gate"]
    fn session_drop_leak_probe() {
        fn env_u64(name: &str, default: u64) -> u64 {
            match std::env::var(name) {
                Ok(v) => v.parse().unwrap_or_else(|_| panic!("{name}={v} is not a number")),
                Err(_) => default,
            }
        }
        let cycles = env_u64("RMC_LEAK_PROBE_CYCLES", 20);
        let min_free = env_u64("RMC_LEAK_PROBE_MIN_FREE_MB", 3072) << 20;
        let total = crate::embeddings::gpu_ledger_report()
            .card_total_bytes
            .expect("no amdgpu sysfs meter");

        let _guard = model_guard();
        let backend = EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        let texts = full_gpu_batch();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let mib = |b: u64| b >> 20;

        let start = read_vram();
        eprintln!(
            "start: process {} MiB, card {} / {} MiB",
            mib(start.process_bytes.unwrap()),
            mib(start.card_used_bytes.unwrap()),
            mib(total)
        );
        for cycle in 1..=cycles {
            let free = total - read_vram().card_used_bytes.unwrap();
            if free < min_free {
                eprintln!("stop before cycle {cycle}: card free {} MiB < {} MiB", mib(free), mib(min_free));
                break;
            }
            let embedder = FastembedOnnxEmbedder::new_gpu(&backend, EmbeddingOwner::Tool, &Admission::for_probe()).unwrap();
            {
                let _gpu = crate::embeddings::gpu_lock::GpuLock::acquire().unwrap();
                embedder.embed_documents(&refs).unwrap();
            }
            let loaded = read_vram();
            drop(embedder);
            let after = read_vram();
            let totals = owner_totals(EmbeddingOwner::Tool);
            eprintln!(
                "cycle {cycle:2}: loaded process {} MiB → after drop {} MiB, card {} MiB; \
                 total charged {} MiB, returned {} MiB, kept {} MiB",
                mib(loaded.process_bytes.unwrap()),
                mib(after.process_bytes.unwrap()),
                mib(after.card_used_bytes.unwrap()),
                totals.released_charge.process_bytes.unwrap() >> 20,
                totals.released_returned.process_bytes.unwrap() >> 20,
                totals.released_kept.process_bytes.unwrap() >> 20,
            );
        }
    }

    /// Positive control for the oracle above: on the CPU profile the census must
    /// show CPU and ZERO nodes on MIGraphX.
    ///
    /// Without it a 'green GPU test' proves nothing: a test that is green both
    /// when everything runs on GPU and when everything runs on CPU is not a gate but
    /// decoration. Here the same machinery (profile → census) runs on a
    /// known-CPU session, and the assertion is exactly the opposite — this shows that
    /// the census DISTINGUISHES the two outcomes rather than always saying 'yes'.
    ///
    /// No GPU needed, only a downloaded model — hence `#[ignore]`:
    /// `cargo test -p rmc-engine --features embeddings census_on_cpu -- --ignored --nocapture`
    #[test]
    #[ignore = "downloads the model from HF and runs forward on CPU"]
    fn census_on_cpu_profile_sees_no_migraphx() {
        use crate::embeddings::ep_census::{CPU_EP, MIGRAPHX_EP};

        let _guard = model_guard();
        let backend = EmbeddingBackend::from_profile_name("local-cpu-small").unwrap();

        let census = probe_provider_census(&backend).unwrap();
        eprintln!("nodes per provider (CPU profile): {census}");
        assert_eq!(census.nodes_on(MIGRAPHX_EP), 0);
        assert!(census.nodes_on(CPU_EP) > 0, "{census}");
    }
}
