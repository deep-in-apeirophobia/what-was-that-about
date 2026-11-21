//! Speech model catalog, on-disk layout, downloads (with resume + checksum) and manual import.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::ggml;
use crate::types::{
    EngineKind, ManualFile, ModelInfo, ModelProgress, ModelProgressStage, ModelStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// sherpa-onnx NeMo CTC bundle: model.onnx + tokens.txt
    SherpaNemoCtc,
    /// Hugging Face Whisper checkpoint converted locally to whisper.cpp GGML
    WhisperHf,
}

#[derive(Debug, Clone)]
pub struct FileSpec {
    pub name: &'static str,
    pub url: String,
    pub size: u64,
    pub sha256: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub engine: EngineKind,
    pub kind: ModelKind,
    pub description: &'static str,
    pub license: &'static str,
    pub recommended_for: &'static str,
    pub homepage: &'static str,
    pub files: Vec<FileSpec>,
    /// Approximate size on disk after install (per output format for Whisper).
    pub disk_bytes: u64,
    /// Decoder cross-attention heads for DTW word timings (Whisper only).
    pub alignment_heads: &'static [(i32, i32)],
}

fn hf(repo: &str, rev: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{rev}/{file}?download=true")
}

const TARANEH_REPO: &str = "areffarhadi/Taraneh-WLarge";
const TARANEH_REV: &str = "876da71621f711fbd41e63310c166614caef0b31";
const SHENAVA_REPO: &str = "Reza2kn/Shenava-Koochik-v1.0-sherpa-onnx";
const SHENAVA_REV: &str = "8ec923790853cc3e516cbca048dfb8a5b10592d5";

pub fn catalog() -> Vec<ModelSpec> {
    vec![
        ModelSpec {
            id: "taraneh-wlarge",
            name: "Taraneh-WLarge",
            engine: EngineKind::Whisper,
            kind: ModelKind::WhisperHf,
            description: "Whisper large-v3 fine-tuned on about 1,560 hours of Persian speech \
                (Aref Farhadipour, University of Zurich). Most accurate option (3.9% WER on FLEURS). \
                Fast on a GPU (Vulkan on Windows/Linux, Metal on macOS); usable but slow on CPU. \
                Downloaded once (6.2 GB) and converted on this computer to a compact whisper.cpp model.",
            license: "MIT",
            recommended_for: "Best accuracy · GPU",
            homepage: "https://huggingface.co/areffarhadi/Taraneh-WLarge",
            files: vec![
                FileSpec { name: "config.json", url: hf(TARANEH_REPO, TARANEH_REV, "config.json"), size: 1399, sha256: None },
                FileSpec { name: "vocab.json", url: hf(TARANEH_REPO, TARANEH_REV, "vocab.json"), size: 1_036_558, sha256: None },
                FileSpec { name: "model.safetensors.index.json", url: hf(TARANEH_REPO, TARANEH_REV, "model.safetensors.index.json"), size: 111_598, sha256: None },
                FileSpec {
                    name: "model-00001-of-00002.safetensors",
                    url: hf(TARANEH_REPO, TARANEH_REV, "model-00001-of-00002.safetensors"),
                    size: 4_993_448_880,
                    sha256: Some("19cf9ebd66d7307ab04fd766bd7bc706830919a109911f2234809563539d9747"),
                },
                FileSpec {
                    name: "model-00002-of-00002.safetensors",
                    url: hf(TARANEH_REPO, TARANEH_REV, "model-00002-of-00002.safetensors"),
                    size: 1_180_663_192,
                    sha256: Some("8416ede7e55692f0bf5adb7357f2d8eaee49e08d150120944ea19aafbb552303"),
                },
            ],
            disk_bytes: 1_660_000_000,
            alignment_heads: &[(23, 3), (26, 7)],
        },
        ModelSpec {
            id: "shenava-koochik",
            name: "Shenava Koochik v1.0",
            engine: EngineKind::Shenava,
            kind: ModelKind::SherpaNemoCtc,
            description: "114M-parameter Persian FastConformer CTC model (Reza2kn / PersianML). \
                Runs fast on any CPU, including laptops (10.6% WER on FLEURS). \
                Good default when there is no capable GPU. 459 MB download.",
            license: "Apache-2.0",
            recommended_for: "CPU · laptops · fast",
            homepage: "https://huggingface.co/Reza2kn/Shenava-Koochik-v1.0",
            files: vec![
                FileSpec {
                    name: "model.onnx",
                    url: hf(SHENAVA_REPO, SHENAVA_REV, "model.onnx"),
                    size: 458_819_249,
                    sha256: Some("6a564b5541920ce1c37bbc91d22e4b3a6838648b9b327eb88997e8db1f90950d"),
                },
                FileSpec { name: "tokens.txt", url: hf(SHENAVA_REPO, SHENAVA_REV, "tokens.txt"), size: 12_236, sha256: None },
            ],
            disk_bytes: 458_831_485,
            alignment_heads: &[],
        },
    ]
}

pub fn spec(id: &str) -> Option<ModelSpec> {
    catalog().into_iter().find(|m| m.id == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledManifest {
    pub version: u32,
    /// Main model file, relative to the model directory.
    pub model_file: String,
    /// Extra files (relative), e.g. tokens.txt
    #[serde(default)]
    pub extra_files: Vec<String>,
    pub format: String,
    pub installed_at: String,
    /// true when installed from user-provided files
    #[serde(default)]
    pub manual: bool,
}

pub struct ModelPaths {
    pub dir: PathBuf,
}

impl ModelPaths {
    pub fn new(models_root: &Path, id: &str) -> Self {
        Self {
            dir: models_root.join(id),
        }
    }
    pub fn manifest(&self) -> PathBuf {
        self.dir.join("installed.json")
    }
    pub fn source_dir(&self) -> PathBuf {
        self.dir.join("source")
    }
}

pub fn read_manifest(models_root: &Path, id: &str) -> Option<InstalledManifest> {
    let p = ModelPaths::new(models_root, id);
    let m: InstalledManifest = serde_json::from_slice(&std::fs::read(p.manifest()).ok()?).ok()?;
    let ok = p.dir.join(&m.model_file).is_file()
        && m.extra_files.iter().all(|f| p.dir.join(f).is_file());
    ok.then_some(m)
}

pub fn write_manifest(models_root: &Path, id: &str, m: &InstalledManifest) -> Result<()> {
    let p = ModelPaths::new(models_root, id);
    std::fs::create_dir_all(&p.dir)?;
    std::fs::write(p.manifest(), serde_json::to_vec_pretty(m)?)?;
    Ok(())
}

fn manual_instructions(spec: &ModelSpec) -> String {
    match spec.kind {
        ModelKind::SherpaNemoCtc => format!(
            "Download `model.onnx` and `tokens.txt` from the links below (or from the \
             [model page]({}/tree/main)) on any computer, then click **Import files…** and select \
             both files. No internet connection is needed for the import.",
            spec.homepage.replace("Shenava-Koochik-v1.0", "Shenava-Koochik-v1.0-sherpa-onnx")
        ),
        ModelKind::WhisperHf => format!(
            "Download `config.json`, `vocab.json`, `model.safetensors.index.json` and both \
             `model-0000X-of-00002.safetensors` files from the links below (or the \
             [model page]({}/tree/main)) into one folder, then click **Import files…** and select \
             all of them. The app converts them for whisper.cpp (needs ~2 GB free space and a few \
             minutes). Alternatively, select a ready-made whisper.cpp `ggml-*.bin` file of this model.",
            spec.homepage
        ),
    }
}

pub fn model_info(
    models_root: &Path,
    spec: &ModelSpec,
    status_override: Option<(ModelStatus, Option<String>)>,
) -> ModelInfo {
    let installed = read_manifest(models_root, spec.id).is_some();
    let (status, error) = match status_override {
        Some(s) => s,
        None if installed => (ModelStatus::Ready, None),
        None => (ModelStatus::NotInstalled, None),
    };
    ModelInfo {
        id: spec.id.into(),
        name: spec.name.into(),
        engine: spec.engine,
        description: spec.description.into(),
        license: spec.license.into(),
        recommended_for: spec.recommended_for.into(),
        download_bytes: spec.files.iter().map(|f| f.size).sum(),
        disk_bytes: spec.disk_bytes,
        homepage: spec.homepage.into(),
        status,
        error,
        install_dir: ModelPaths::new(models_root, spec.id)
            .dir
            .to_string_lossy()
            .into_owned(),
        manual_files: spec
            .files
            .iter()
            .map(|f| ManualFile {
                name: f.name.into(),
                url: f.url.clone(),
                size_bytes: Some(f.size),
                required: true,
            })
            .collect(),
        manual_instructions: manual_instructions(spec),
    }
}

pub type ProgressFn<'a> = dyn FnMut(ModelProgress) + Send + 'a;

struct Throttle {
    last: Instant,
}
impl Throttle {
    fn new() -> Self {
        Self {
            last: Instant::now() - Duration::from_secs(1),
        }
    }
    fn ready(&mut self) -> bool {
        if self.last.elapsed() >= Duration::from_millis(200) {
            self.last = Instant::now();
            true
        } else {
            false
        }
    }
}

pub fn http_client() -> Result<reqwest::Client> {
    crate::ensure_crypto_provider();
    Ok(reqwest::Client::builder()
        .user_agent(concat!("what-was-that-about/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()?)
}

pub fn sha256_file(path: &Path, cancel: &AtomicBool) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Download one file with resume and retries. `base_done` is added to progress so a
/// multi-file install reports one overall number.
#[allow(clippy::too_many_arguments)]
pub async fn download_file(
    client: &reqwest::Client,
    model_id: &str,
    file: &FileSpec,
    dest: &Path,
    base_done: u64,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> Result<()> {
    if dest.is_file() && std::fs::metadata(dest)?.len() == file.size {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = dest.with_file_name(format!("{}.part", file.name));
    let mut throttle = Throttle::new();
    let mut attempt = 0u32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let have = tokio::fs::metadata(&part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if have == file.size {
            break;
        }
        let mut req = client.get(&file.url);
        if have > 0 && have < file.size {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let result: Result<()> = async {
            let resp = req.send().await?.error_for_status()?;
            let resumed = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            let mut out = if resumed {
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&part)
                    .await?
            } else {
                tokio::fs::File::create(&part).await?
            };
            let mut done = if resumed { have } else { 0 };
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                if cancel.load(Ordering::Relaxed) {
                    out.flush().await?;
                    bail!("cancelled");
                }
                let chunk = chunk?;
                out.write_all(&chunk).await?;
                done += chunk.len() as u64;
                if throttle.ready() {
                    progress(ModelProgress {
                        model_id: model_id.into(),
                        stage: ModelProgressStage::Downloading,
                        file: Some(file.name.into()),
                        done_bytes: base_done + done,
                        total_bytes: total,
                        message: None,
                    });
                }
            }
            out.flush().await?;
            if done != file.size {
                bail!("connection closed early ({done} of {} bytes)", file.size);
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => break,
            Err(e) if e.to_string() == "cancelled" => return Err(e),
            Err(e) => {
                attempt += 1;
                if attempt > 8 {
                    return Err(e.context(format!("downloading {}", file.name)));
                }
                log::warn!(
                    "download of {} failed (attempt {attempt}): {e:#}; retrying",
                    file.name
                );
                progress(ModelProgress {
                    model_id: model_id.into(),
                    stage: ModelProgressStage::Downloading,
                    file: Some(file.name.into()),
                    done_bytes: base_done + have,
                    total_bytes: total,
                    message: Some(format!("Connection problem, retrying ({attempt}/8)…")),
                });
                tokio::time::sleep(Duration::from_secs((2u64 << attempt.min(4)).min(30))).await;
            }
        }
    }
    if let Some(expected) = file.sha256 {
        progress(ModelProgress {
            model_id: model_id.into(),
            stage: ModelProgressStage::Verifying,
            file: Some(file.name.into()),
            done_bytes: base_done + file.size,
            total_bytes: total,
            message: None,
        });
        let part2 = part.clone();
        let got = tokio::task::block_in_place(|| sha256_file(&part2, cancel))?;
        if got != expected {
            let _ = tokio::fs::remove_file(&part).await;
            bail!(
                "checksum mismatch for {} (the download was corrupted, please retry)",
                file.name
            );
        }
    }
    tokio::fs::rename(&part, dest).await?;
    Ok(())
}

/// Install a model from the internet: download every file, then convert if needed.
pub async fn install(
    models_root: &Path,
    spec: &ModelSpec,
    whisper_out: ggml::OutType,
    keep_source: bool,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> Result<()> {
    let paths = ModelPaths::new(models_root, spec.id);
    let client = http_client()?;
    let total: u64 = spec.files.iter().map(|f| f.size).sum();
    let target_dir = match spec.kind {
        ModelKind::SherpaNemoCtc => paths.dir.clone(),
        ModelKind::WhisperHf => paths.source_dir(),
    };
    let mut done = 0u64;
    for f in &spec.files {
        download_file(
            &client,
            spec.id,
            f,
            &target_dir.join(f.name),
            done,
            total,
            cancel,
            progress,
        )
        .await?;
        done += f.size;
    }
    match spec.kind {
        ModelKind::SherpaNemoCtc => {
            write_manifest(
                models_root,
                spec.id,
                &InstalledManifest {
                    version: 1,
                    model_file: "model.onnx".into(),
                    extra_files: vec!["tokens.txt".into()],
                    format: "onnx".into(),
                    installed_at: crate::types::now_iso(),
                    manual: false,
                },
            )?;
        }
        ModelKind::WhisperHf => {
            let src = paths.source_dir();
            let sources = ggml::SourceFiles::from_dir(&src)?;
            convert_whisper(
                models_root,
                spec,
                &sources,
                whisper_out,
                false,
                cancel,
                progress,
            )?;
            if !keep_source {
                let _ = std::fs::remove_dir_all(&src);
            }
        }
    }
    Ok(())
}

pub fn convert_whisper(
    models_root: &Path,
    spec: &ModelSpec,
    sources: &ggml::SourceFiles,
    out_type: ggml::OutType,
    manual: bool,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> Result<()> {
    let paths = ModelPaths::new(models_root, spec.id);
    std::fs::create_dir_all(&paths.dir)?;
    let out = paths.dir.join(out_type.file_name());
    let mut throttle = Throttle::new();
    let id = spec.id;
    tokio::task::block_in_place(|| {
        ggml::convert(sources, &out, out_type, cancel, &mut |p| {
            if throttle.ready() || p.done_bytes == p.total_bytes {
                progress(ModelProgress {
                    model_id: id.into(),
                    stage: ModelProgressStage::Converting,
                    file: Some(out_type.file_name().into()),
                    done_bytes: p.done_bytes,
                    total_bytes: p.total_bytes,
                    message: None,
                });
            }
        })
    })?;
    write_manifest(
        models_root,
        spec.id,
        &InstalledManifest {
            version: 1,
            model_file: out_type.file_name().into(),
            extra_files: vec![],
            format: match out_type {
                ggml::OutType::F16 => "ggml-f16".into(),
                ggml::OutType::Q8_0 => "ggml-q8_0".into(),
            },
            installed_at: crate::types::now_iso(),
            manual,
        },
    )?;
    // remove other formats to free space
    for other in [ggml::OutType::F16, ggml::OutType::Q8_0] {
        if other != out_type {
            let _ = std::fs::remove_file(paths.dir.join(other.file_name()));
        }
    }
    Ok(())
}

fn file_name_lower(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Install from files the user downloaded manually (offline-friendly).
pub fn import_files(
    models_root: &Path,
    spec: &ModelSpec,
    paths_in: &[PathBuf],
    whisper_out: ggml::OutType,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> Result<()> {
    let paths = ModelPaths::new(models_root, spec.id);
    std::fs::create_dir_all(&paths.dir)?;
    match spec.kind {
        ModelKind::SherpaNemoCtc => {
            let onnx = paths_in
                .iter()
                .filter(|p| file_name_lower(p).ends_with(".onnx"))
                .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
                .ok_or_else(|| anyhow!("select the model's .onnx file (model.onnx)"))?;
            let tokens = paths_in
                .iter()
                .find(|p| file_name_lower(p) == "tokens.txt")
                .ok_or_else(|| anyhow!("select tokens.txt together with the .onnx file"))?;
            let total = std::fs::metadata(onnx)?.len();
            progress(ModelProgress {
                model_id: spec.id.into(),
                stage: ModelProgressStage::Verifying,
                file: Some("model.onnx".into()),
                done_bytes: 0,
                total_bytes: total,
                message: Some("Copying model files…".into()),
            });
            copy_into(onnx, &paths.dir.join("model.onnx"))?;
            copy_into(tokens, &paths.dir.join("tokens.txt"))?;
            write_manifest(
                models_root,
                spec.id,
                &InstalledManifest {
                    version: 1,
                    model_file: "model.onnx".into(),
                    extra_files: vec!["tokens.txt".into()],
                    format: "onnx".into(),
                    installed_at: crate::types::now_iso(),
                    manual: true,
                },
            )?;
            Ok(())
        }
        ModelKind::WhisperHf => {
            if let Some(g) = paths_in.iter().find(|p| ggml::looks_like_ggml(p)) {
                copy_into(g, &paths.dir.join("ggml-model-custom.bin"))?;
                write_manifest(
                    models_root,
                    spec.id,
                    &InstalledManifest {
                        version: 1,
                        model_file: "ggml-model-custom.bin".into(),
                        extra_files: vec![],
                        format: "ggml".into(),
                        installed_at: crate::types::now_iso(),
                        manual: true,
                    },
                )?;
                return Ok(());
            }
            let find = |name: &str| {
                paths_in
                    .iter()
                    .find(|p| file_name_lower(p) == name)
                    .cloned()
            };
            let config = find("config.json")
                .ok_or_else(|| anyhow!("config.json is missing from the selection"))?;
            let vocab = find("vocab.json")
                .ok_or_else(|| anyhow!("vocab.json is missing from the selection"))?;
            let mut shards: Vec<PathBuf> = paths_in
                .iter()
                .filter(|p| file_name_lower(p).ends_with(".safetensors"))
                .cloned()
                .collect();
            if shards.is_empty() {
                bail!("select the .safetensors weight files");
            }
            shards.sort();
            if let Some(index) = find("model.safetensors.index.json") {
                let needed = ggml::SourceFiles::shard_names_from_index(&index)?;
                for n in &needed {
                    if !shards
                        .iter()
                        .any(|s| file_name_lower(s) == n.to_lowercase())
                    {
                        bail!("{n} is missing from the selection");
                    }
                }
            }
            for f in &spec.files {
                if let Some(expected) = f.sha256 {
                    if let Some(p) = shards.iter().find(|s| file_name_lower(s) == f.name) {
                        if std::fs::metadata(p)?.len() != f.size {
                            bail!(
                                "{} has the wrong size; the download may be incomplete",
                                f.name
                            );
                        }
                        progress(ModelProgress {
                            model_id: spec.id.into(),
                            stage: ModelProgressStage::Verifying,
                            file: Some(f.name.into()),
                            done_bytes: 0,
                            total_bytes: f.size,
                            message: Some("Checking file integrity…".into()),
                        });
                        if sha256_file(p, cancel)? != expected {
                            bail!(
                                "{} is corrupted (checksum mismatch); please download it again",
                                f.name
                            );
                        }
                    }
                }
            }
            let sources = ggml::SourceFiles {
                config,
                vocab,
                shards,
            };
            convert_whisper(
                models_root,
                spec,
                &sources,
                whisper_out,
                true,
                cancel,
                progress,
            )
        }
    }
}

fn copy_into(src: &Path, dst: &Path) -> Result<()> {
    if src == dst {
        return Ok(());
    }
    let tmp = dst.with_extension("part");
    std::fs::copy(src, &tmp).with_context(|| format!("copying {}", src.display()))?;
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

pub fn delete(models_root: &Path, id: &str) -> Result<()> {
    let p = ModelPaths::new(models_root, id);
    if p.dir.exists() {
        std::fs::remove_dir_all(&p.dir)?;
    }
    Ok(())
}

/// Absolute path of the installed model's main file.
pub fn model_file(models_root: &Path, id: &str) -> Option<PathBuf> {
    let m = read_manifest(models_root, id)?;
    Some(ModelPaths::new(models_root, id).dir.join(m.model_file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_both_models() {
        let c = catalog();
        assert!(c
            .iter()
            .any(|m| m.id == "taraneh-wlarge" && m.engine == EngineKind::Whisper));
        assert!(c
            .iter()
            .any(|m| m.id == "shenava-koochik" && m.engine == EngineKind::Shenava));
        for m in c {
            assert!(m
                .files
                .iter()
                .all(|f| f.url.starts_with("https://huggingface.co/")));
        }
    }

    #[test]
    fn manual_import_shenava_and_manifest() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let onnx = src.path().join("model.onnx");
        let tok = src.path().join("tokens.txt");
        std::fs::write(&onnx, b"fake").unwrap();
        std::fs::write(&tok, b"<blk> 0").unwrap();
        let spec = spec("shenava-koochik").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(read_manifest(root.path(), spec.id).is_none());
        import_files(
            root.path(),
            &spec,
            &[onnx, tok],
            ggml::OutType::Q8_0,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        let m = read_manifest(root.path(), spec.id).unwrap();
        assert!(m.manual);
        assert_eq!(
            model_info(root.path(), &spec, None).status,
            ModelStatus::Ready
        );
        delete(root.path(), spec.id).unwrap();
        assert_eq!(
            model_info(root.path(), &spec, None).status,
            ModelStatus::NotInstalled
        );
    }

    #[test]
    fn manual_import_rejects_incomplete_whisper() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let cfg = src.path().join("config.json");
        std::fs::write(&cfg, b"{}").unwrap();
        let spec = spec("taraneh-wlarge").unwrap();
        let cancel = AtomicBool::new(false);
        let err = import_files(
            root.path(),
            &spec,
            &[cfg],
            ggml::OutType::Q8_0,
            &cancel,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("vocab.json"));
    }
}
