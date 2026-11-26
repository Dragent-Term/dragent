use std::path::PathBuf;

use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// The embedding model. We use the **full-precision** BAAI/bge-small-en-v1.5
/// (384 dims, ~130MB) rather than the quantized `BGESmallENV15Q` (~33MB):
/// the quantized ONNX graph fuses LayerNorm into SkipLayerNormalization but drops
/// a required weight, so inference crashes under this onnxruntime with
/// "Missing Input: encoder.layer.0.attention.output.LayerNorm.weight". The
/// full-precision model has no such fusion issue. If you change this, bump
/// `VectorStore::SCHEMA_VERSION` so stale embeddings are rebuilt.
const EMBEDDING_MODEL: EmbeddingModel = EmbeddingModel::BGESmallENV15;
const INFERENCE_BATCH_SIZE: usize = 8;

/// Suppress the child's console window on Windows. No-op elsewhere.
///
/// dragent.exe is a GUI-subsystem binary and owns no console, so every console
/// child (curl, tar) otherwise pops its own window for the life of the process.
fn no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

pub struct Embedder {
    model: TextEmbedding,
}

/// Minimal embedding surface consumed by the indexing batches. The production
/// [`Embedder`] implements it; tests inject a fake that counts calls, proving
/// a second worktree of the same repo re-embeds nothing unchanged.
pub trait EmbedText {
    fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}

impl EmbedText for Embedder {
    fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.embed(texts)
    }
}

impl EmbedText for dragent_resource_lifecycle::ResourceLease<Embedder> {
    fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.embed(texts)
    }
}

impl Embedder {
    /// Load the embedding model (downloads to the model cache dir on first use).
    pub fn load() -> Result<Self> {
        Self::configure_ort_dylib();

        let cache_dir = Self::cache_dir();
        std::fs::create_dir_all(&cache_dir)?;

        let model = TextEmbedding::try_new(
            InitOptions::new(EMBEDDING_MODEL)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(false)
                // 256 tokens covers our ~500-char code chunks with headroom;
                // halving the 512 default quarters the attention buffers
                // (quadratic in sequence length) that ort's arena retains.
                .with_max_length(256)
                .with_execution_providers(Self::execution_providers()),
        )?;

        Ok(Self { model })
    }

    /// Execution providers to request, most-preferred first. Session creation
    /// registers them in order and ONNX Runtime falls back to CPU for any that
    /// the loaded library doesn't support, so this can only speed things up —
    /// never break inference. `DRAGENT_EMBED_EP` pins one explicitly:
    /// `cpu` (skip acceleration), `qnn` (Snapdragon NPU), or `dml`/`directml`
    /// (any Windows GPU via DirectML); an explicit choice is registered with
    /// `error_on_failure` so a misconfigured deployment fails loudly instead of
    /// silently burning CPU.
    fn execution_providers() -> Vec<ort::execution_providers::ExecutionProviderDispatch> {
        use ort::execution_providers::{
            CoreMLExecutionProvider, DirectMLExecutionProvider, QNNExecutionProvider,
        };

        let choice = std::env::var("DRAGENT_EMBED_EP")
            .unwrap_or_default()
            .to_ascii_lowercase();
        match choice.as_str() {
            "cpu" => {
                log::info!("[index] embedding on CPU (DRAGENT_EMBED_EP=cpu)");
                vec![]
            }
            "qnn" => {
                log::info!("[index] embedding via QNN (pinned)");
                vec![QNNExecutionProvider::default().build().error_on_failure()]
            }
            "dml" | "directml" => {
                log::info!("[index] embedding via DirectML (pinned)");
                vec![
                    DirectMLExecutionProvider::default()
                        .build()
                        .error_on_failure(),
                ]
            }
            "coreml" => {
                log::info!("[index] embedding via CoreML (pinned)");
                vec![
                    CoreMLExecutionProvider::default()
                        .build()
                        .error_on_failure(),
                ]
            }
            _ => {
                // `mut` is unused on targets where both pushes below are cfg'd out.
                #[allow(unused_mut)]
                let mut eps = Vec::new();
                // Snapdragon NPU first — an order of magnitude better perf/watt
                // for this model when the runtime has the QNN EP compiled in.
                #[cfg(all(windows, target_arch = "aarch64"))]
                eps.push(QNNExecutionProvider::default().build());
                // Any Windows GPU next.
                #[cfg(windows)]
                eps.push(DirectMLExecutionProvider::default().build());
                // CoreML is deliberately NOT requested by default on macOS:
                // fastembed pads each batch to its longest sequence, so nearly
                // every batch has a fresh input shape, and ort's CoreML EP
                // compiles + caches a model per shape. Measured on a 1k-file
                // initial pass: 4.7 GB peak RSS with CoreML vs 0.67 GB on CPU
                // (same wall-clock) — the memory pressure froze the whole
                // desktop. BGE-small on CPU at batch 8 is fast enough; pin
                // DRAGENT_EMBED_EP=coreml to opt in explicitly.
                if !eps.is_empty() {
                    log::info!(
                        "[index] embedding acceleration requested (QNN/DirectML as available; CPU fallback)"
                    );
                }
                eps
            }
        }
    }

    /// Embed a batch of *passages* (document chunks). Returns one 384-dim
    /// L2-normalised vector per input. No instruction is prepended — BGE-v1.5
    /// wants passages bare.
    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        Ok(self.model.embed(texts, Some(INFERENCE_BATCH_SIZE))?)
    }

    /// Embed a *search query*. BGE-v1.5 is an asymmetric retriever: queries must
    /// carry this instruction while passages get none. fastembed does not add it
    /// automatically, so without this the query and passage vectors sit in
    /// slightly different spaces and ranking quality drops. Passages must go
    /// through [`Self::embed`].
    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        const BGE_QUERY_INSTRUCTION: &str =
            "Represent this sentence for searching relevant passages: ";
        let prompt = format!("{BGE_QUERY_INSTRUCTION}{query}");
        let mut v = self.model.embed(vec![prompt], Some(INFERENCE_BATCH_SIZE))?;
        if v.is_empty() {
            anyhow::bail!("embedder returned no vector for query");
        }
        Ok(v.remove(0))
    }

    pub fn cache_dir() -> PathBuf {
        dragent_paths::platform_support().join("models")
    }

    /// ort is built with `load-dynamic` (pyke publishes no dylib for
    /// aarch64-pc-windows-msvc and static linking fails there), so ONNX Runtime
    /// is loaded from a shared library at runtime. Left to its default, ort
    /// resolves the bare platform name (`onnxruntime.dll`, …) through the OS
    /// search path — on Windows 11 that finds the OS's own Windows ML copy in
    /// System32 (1.17.x on ARM64), which is older than ort supports and panics.
    /// Point ort at a known-good copy before its first use: `ORT_DYLIB_PATH` if
    /// the user set it, else a library next to the executable, else one in the
    /// model cache dir. With no candidate, fall through to ort's default search
    /// so platforms with a resolvable system library keep working.
    fn configure_ort_dylib() {
        if std::env::var("ORT_DYLIB_PATH").is_ok_and(|v| !v.is_empty()) {
            return; // explicit override — ort reads this env var itself
        }

        const DYLIB_NAME: &str = if cfg!(windows) {
            "onnxruntime.dll"
        } else if cfg!(target_vendor = "apple") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };

        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let candidates = exe_dir
            .into_iter()
            .chain(std::iter::once(Self::cache_dir()))
            .map(|dir| dir.join(DYLIB_NAME));

        for path in candidates {
            if path.is_file() {
                log::info!("[index] using ONNX Runtime at {}", path.display());
                // Seeds ort's dylib path (first call wins); the environment
                // itself is created by fastembed, so the builder is dropped.
                let _ = ort::init_from(path.to_string_lossy());
                return;
            }
        }

        // No local copy anywhere. Provision the pinned official release into
        // the model cache dir (the same download-on-first-use treatment the
        // embedding model itself gets). On macOS the OS search can never
        // succeed — Apple ships no ONNX Runtime. On Windows the OS search
        // finds whatever System32/PATH has, which is a lottery: Windows ML's
        // System32 copy is 1.17.x on ARM64 and panics under this ort. A
        // pinned copy makes every machine deterministic; if the download
        // fails (offline), the OS search below still runs as a last resort.
        #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
        match Self::provision_dylib() {
            Ok(path) => {
                log::info!(
                    "[index] using provisioned ONNX Runtime at {}",
                    path.display()
                );
                let _ = ort::init_from(path.to_string_lossy());
                return;
            }
            Err(e) => {
                log::warn!("[index] could not provision ONNX Runtime: {e:#}");
            }
        }

        log::info!("[index] no bundled ONNX Runtime found — using OS search for {DYLIB_NAME}");
    }

    /// Download the pinned official ONNX Runtime release for this platform
    /// into the model cache dir and return the library path. Idempotent: the
    /// extracted library is found by the candidate scan on subsequent
    /// launches, so this runs at most once per machine. Runs on the
    /// embedder's background thread — never on the UI thread.
    ///
    /// Uses `curl` + `tar`, which ship with both macOS and Windows 10+
    /// (bsdtar on Windows extracts .zip too) — no HTTP client dependency.
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    fn provision_dylib() -> Result<PathBuf> {
        use sha2::{Digest, Sha256};

        // Must match the ONNX Runtime minor version `ort` targets
        // (ort 2.0.0-rc.9 → API version 20 → 1.20.x).
        const ORT_VERSION: &str = "1.20.1";

        // Per-platform release facts. SHA-256 values are of the official
        // GitHub release assets, verified at pin time.
        #[cfg(target_os = "macos")]
        const PLATFORM: ProvisionSpec = ProvisionSpec {
            asset_stem: if cfg!(target_arch = "aarch64") {
                "onnxruntime-osx-arm64-1.20.1"
            } else {
                "onnxruntime-osx-x86_64-1.20.1"
            },
            asset_ext: "tgz",
            sha256: if cfg!(target_arch = "aarch64") {
                "b678fc3c2354c771fea4fba420edeccfba205140088334df801e7fc40e83a57a"
            } else {
                "0f73006813af2a1a5d1723ed7dfb694fc629d15037124081bb61b7bf7d99fc78"
            },
            // The bare-named entry in the tarball is a symlink; extract the
            // real versioned file and install it under the canonical name.
            libs: &[("lib/libonnxruntime.1.20.1.dylib", "libonnxruntime.dylib")],
            approx_size: "9 MB",
        };
        #[cfg(target_os = "linux")]
        const PLATFORM: ProvisionSpec = ProvisionSpec {
            asset_stem: if cfg!(target_arch = "aarch64") {
                "onnxruntime-linux-aarch64-1.20.1"
            } else {
                "onnxruntime-linux-x64-1.20.1"
            },
            asset_ext: "tgz",
            sha256: if cfg!(target_arch = "aarch64") {
                "ae4fedbdc8c18d688c01306b4b50c63de3445cdf2dbd720e01a2fa3810b8106a"
            } else {
                "67db4dc1561f1e3fd42e619575c82c601ef89849afc7ea85a003abbac1a1a105"
            },
            // The bare-named entry is a symlink chain; install the real
            // versioned object under the canonical name ort searches for.
            libs: &[("lib/libonnxruntime.so.1.20.1", "libonnxruntime.so")],
            approx_size: "6 MB",
        };
        #[cfg(windows)]
        const PLATFORM: ProvisionSpec = ProvisionSpec {
            asset_stem: if cfg!(target_arch = "aarch64") {
                "onnxruntime-win-arm64-1.20.1"
            } else {
                "onnxruntime-win-x64-1.20.1"
            },
            asset_ext: "zip",
            sha256: if cfg!(target_arch = "aarch64") {
                "21ded41cf725dec14bf138f891438afd06f5363fe8b2477049e7262bd7238ce2"
            } else {
                "78d447051e48bd2e1e778bba378bec4ece11191c9e538cf7b2c4a4565e8f5581"
            },
            // onnxruntime.dll loads onnxruntime_providers_shared.dll from its
            // own directory; install both. This is the CPU/default build —
            // the DirectML/QNN EPs the session requests simply fall back to
            // CPU, exactly like a System32 copy without them.
            libs: &[
                ("lib/onnxruntime.dll", "onnxruntime.dll"),
                (
                    "lib/onnxruntime_providers_shared.dll",
                    "onnxruntime_providers_shared.dll",
                ),
            ],
            approx_size: "63 MB",
        };

        let cache = Self::cache_dir();
        std::fs::create_dir_all(&cache)?;
        let dest = cache.join(PLATFORM.libs[0].1);
        if dest.is_file() {
            return Ok(dest);
        }

        let url = format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/{}.{}",
            PLATFORM.asset_stem, PLATFORM.asset_ext
        );
        let archive = cache.join(format!(
            "{}.{}.partial",
            PLATFORM.asset_stem, PLATFORM.asset_ext
        ));
        log::info!(
            "[index] downloading ONNX Runtime {ORT_VERSION} — one-time, ~{}",
            PLATFORM.approx_size
        );

        let mut curl = std::process::Command::new("curl");
        curl.args(["-fsSL", "--retry", "3", "--max-time", "600", "-o"])
            .arg(&archive)
            .arg(&url);
        no_window(&mut curl);
        let status = curl
            .status()
            .map_err(|e| anyhow::anyhow!("could not run curl: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&archive);
            anyhow::bail!("download failed ({status}) from {url}");
        }

        let bytes = std::fs::read(&archive)?;
        let actual_sha = hex::encode(Sha256::digest(&bytes));
        if actual_sha != PLATFORM.sha256 {
            let _ = std::fs::remove_file(&archive);
            anyhow::bail!(
                "checksum mismatch for {url}: expected {}, got {actual_sha}",
                PLATFORM.sha256
            );
        }

        // Extract just the libraries we install. bsdtar (both platforms)
        // auto-detects .tgz and .zip from the archive itself.
        let mut tar = std::process::Command::new("tar");
        tar.arg("-xf").arg(&archive).arg("-C").arg(&cache);
        for (inner, _) in PLATFORM.libs {
            tar.arg(format!("{}/{}", PLATFORM.asset_stem, inner));
        }
        no_window(&mut tar);
        let status = tar
            .status()
            .map_err(|e| anyhow::anyhow!("could not run tar: {e}"))?;
        let _ = std::fs::remove_file(&archive);
        if !status.success() {
            anyhow::bail!("extract failed ({status})");
        }

        for (inner, name) in PLATFORM.libs {
            let extracted = cache.join(PLATFORM.asset_stem).join(inner);
            let target = cache.join(name);
            std::fs::rename(&extracted, &target)
                .or_else(|_| std::fs::copy(&extracted, &target).map(|_| ()))?;
        }
        // Drop the now-empty extraction tree.
        let _ = std::fs::remove_dir_all(cache.join(PLATFORM.asset_stem));

        log::info!("[index] ONNX Runtime installed at {}", dest.display());
        Ok(dest)
    }
}

/// Facts about one platform's pinned ONNX Runtime release asset.
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
struct ProvisionSpec {
    /// Asset file name without extension (also the directory inside the archive).
    asset_stem: &'static str,
    asset_ext: &'static str,
    sha256: &'static str,
    /// (path inside the archive's top-level dir, installed file name).
    /// The first entry is the library ort loads.
    libs: &'static [(&'static str, &'static str)],
    approx_size: &'static str,
}
