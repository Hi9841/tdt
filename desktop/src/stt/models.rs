use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const HF_BASE: &str = "https://huggingface.co";

#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub size_label: &'static str,
    pub size_bytes: u64,
    pub hf_repo: &'static str,
    /// The repository revision the checksums belong to.
    pub revision: &'static str,
    pub dir_name: &'static str,
    pub files: &'static [ModelFile],
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadPhase {
    Idle,
    Downloading {
        id: String,
        done: u64,
        total: u64,
        file: String,
        file_index: usize,
        file_count: usize,
    },
    Ready {
        id: String,
    },
    Failed {
        id: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub done: u64,
    pub total: u64,
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
}

/// Recommended default: Moondream Parakeet Redux 1.58-bit ternary.
pub const DEFAULT_MODEL_ID: &str = "parakeet-redux";

const REDUX_DIR: &str = "parakeet-redux";
const REDUX_REPO: &str = "moondream/parakeet-redux";

const REDUX_FILES: &[ModelFile] = &[
    ModelFile {
        name: "config.json",
        sha256: "503C653B2E3BB788ADBCB04F5ABDEE532D958686564081BAEED133FF10143F6E",
    },
    ModelFile {
        name: "model.safetensors",
        sha256: "78EC25733EE0D0C1586D1346FC86DB9D0C2E436E3A8AB1D32A82D1BB8F848D21",
    },
    ModelFile {
        name: "ternary.json",
        sha256: "1221C6D3CE901FFE09C089DA758A8DB8B76189F80CFF41C5AFC244FC61E2051D",
    },
    ModelFile {
        name: "tokenizer.json",
        sha256: "BD321B096832A3F270BD3B2A88823957920F1A5C5ADA71114A26EA729D0CBE91",
    },
];

/// NVIDIA Parakeet TDT 0.6B v3 as published for Transformers: 207 errors
/// against Redux's 293 on the 456 English test clips, at about the same
/// speed with int8; 2.5 GB on disk and about 0.8 GB in memory.
const V3_FILES: &[ModelFile] = &[
    ModelFile {
        name: "config.json",
        sha256: "E747B85E1BDFD300C8B8AC63BAC8DD5221F8FE9BC275B48D06C735FCD6971B6E",
    },
    ModelFile {
        name: "model.safetensors",
        sha256: "3A2026366188C8C68598EDBBFF92F8D11590A08E0AE2E6775544E7B07D6A5E11",
    },
    ModelFile {
        name: "tokenizer.json",
        sha256: "BD321B096832A3F270BD3B2A88823957920F1A5C5ADA71114A26EA729D0CBE91",
    },
];

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: DEFAULT_MODEL_ID,
        label: "Parakeet Redux",
        size_label: "178 MB",
        size_bytes: 179_005_408,
        hf_repo: REDUX_REPO,
        revision: "main",
        dir_name: REDUX_DIR,
        files: REDUX_FILES,
    },
    ModelSpec {
        id: "parakeet-tdt-0.6b-v3",
        label: "Parakeet v3",
        size_label: "2.5 GB",
        size_bytes: 2_509_472_233,
        hf_repo: "nvidia/parakeet-tdt-0.6b-v3",
        revision: "541d1f99c6b0c3cd0b11a95167540bb8edefd82b",
        dir_name: "parakeet-tdt-0.6b-v3",
        files: V3_FILES,
    },
];

pub const DEFAULT: &ModelSpec = &CATALOG[0];

pub fn by_id(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|spec| spec.id == id.trim())
}

/// Retired ids (INT8, Q8, Whisper, Moonshine, SenseVoice) resolve to Parakeet Redux.
pub fn resolve(id: &str) -> &'static ModelSpec {
    by_id(id).unwrap_or(DEFAULT)
}

impl ModelSpec {
    pub fn dir_in(&self, root: &Path) -> PathBuf {
        root.join(self.dir_name)
    }

    pub fn is_installed_in(&self, dir: &Path) -> bool {
        self.files.iter().all(|file| dir.join(file.name).is_file())
    }

    pub fn require_installed(&self, dir: &Path) -> Result<(), String> {
        for file in self.files {
            let path = dir.join(file.name);
            if !path.is_file() {
                return Err(format!(
                    "{} is missing {} at {}",
                    self.label,
                    file.name,
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

pub fn find_dir(spec: &ModelSpec, override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        if spec.is_installed_in(dir) {
            return Some(canonicalize_or_clone(dir));
        }
    }

    if let Some(dir) = std::env::var_os("VOICE_STT_MODEL_DIR") {
        let dir = PathBuf::from(dir);
        if spec.is_installed_in(&dir) {
            return Some(canonicalize_or_clone(&dir));
        }
    }

    for root in model_roots() {
        let dir = spec.dir_in(&root);
        if spec.is_installed_in(&dir) {
            return Some(canonicalize_or_clone(&dir));
        }
    }

    None
}

pub fn install_dir(spec: &ModelSpec) -> PathBuf {
    if let Some(existing) = find_dir(spec, None) {
        return existing;
    }
    spec.dir_in(&preferred_root())
}

pub fn format_mb(bytes: u64) -> String {
    format!("{} MB", (bytes + 512 * 1024) / (1024 * 1024))
}

pub fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        0
    } else {
        done.saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
            .min(100) as u8
    }
}

pub fn file_label(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".safetensors")
        .trim_end_matches(".json")
        .trim_end_matches(".onnx")
        .trim_end_matches(".txt")
        .replace(".int8", "")
}

pub fn download(
    spec: &ModelSpec,
    dest_dir: &Path,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<(), String> {
    fs::create_dir_all(dest_dir)
        .map_err(|error| format!("Could not create model folder: {error}"))?;

    let total = spec.size_bytes.max(1);
    let file_count = spec.files.len().max(1);
    let mut done = 0u64;

    for (index, file) in spec.files.iter().enumerate() {
        let file_index = index + 1;
        if file.sha256.is_empty() {
            return Err(format!(
                "Model file {} has no checksum; refusing an unverified download",
                file.name
            ));
        }
        on_progress(DownloadProgress {
            done,
            total,
            file: file.name.to_string(),
            file_index,
            file_count,
        });
        let dest = dest_dir.join(file.name);
        if dest.is_file() && file_sha256(&dest)?.eq_ignore_ascii_case(file.sha256) {
            done = (done + file_len(&dest)).min(total);
            on_progress(DownloadProgress {
                done,
                total,
                file: file.name.to_string(),
                file_index,
                file_count,
            });
            continue;
        }

        let url = format!(
            "{HF_BASE}/{}/resolve/{}/{}?download=true",
            spec.hf_repo, spec.revision, file.name
        );
        download_file(&url, &dest, file.sha256, &mut |chunk| {
            done = (done + chunk).min(total);
            on_progress(DownloadProgress {
                done,
                total,
                file: file.name.to_string(),
                file_index,
                file_count,
            });
        })?;
    }

    spec.require_installed(dest_dir)?;
    let last = spec.files.last().map_or("", |file| file.name);
    on_progress(DownloadProgress {
        done: total,
        total,
        file: last.to_string(),
        file_index: file_count,
        file_count,
    });
    Ok(())
}

fn download_file(
    url: &str,
    dest: &Path,
    expected_sha: &str,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..3 {
        match download_file_once(url, dest, expected_sha, &mut *on_chunk) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error;
                eprintln!(
                    "Model download attempt {} failed: {last_error}",
                    attempt + 1
                );
                std::thread::sleep(std::time::Duration::from_millis(500 * (attempt as u64 + 1)));
            }
        }
    }
    Err(last_error)
}

fn download_file_once(
    url: &str,
    dest: &Path,
    expected_sha: &str,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let tmp = dest.with_file_name(format!(
        "{}.download",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }

    let response = model_download_agent()
        .get(url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|error| format!("Download failed: {error}"))?;

    let mut file =
        File::create(&tmp).map_err(|error| format!("Could not write model file: {error}"))?;
    let mut hasher = Sha256::new();
    let mut reader = response.into_reader();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|error| format!("Download failed: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        file.write_all(&buf[..read])
            .map_err(|error| format!("Could not write model file: {error}"))?;
        on_chunk(read as u64);
    }
    drop(file);

    let actual = format!("{:X}", hasher.finalize());
    if !expected_sha.is_empty() && !actual.eq_ignore_ascii_case(expected_sha) {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "SHA256 mismatch for {}",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
    }

    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(&tmp, dest).map_err(|error| format!("Could not finalize model file: {error}"))?;
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("Could not read model file: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|error| format!("Could not read model file: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:X}", hasher.finalize()))
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |meta| meta.len())
}

/// Shared long-read HTTP agent: model downloads and update downloads both need
/// a generous read timeout for large files.
#[doc(hidden)]
pub fn download_agent(user_agent: &str) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(7200))
        .timeout_write(Duration::from_secs(60))
        .user_agent(user_agent)
        .build()
}

fn model_download_agent() -> ureq::Agent {
    download_agent(&format!(
        "TDT/{} (+https://github.com/Hi9841/tdt)",
        env!("CARGO_PKG_VERSION")
    ))
}

fn preferred_root() -> PathBuf {
    for root in model_roots() {
        if root.is_dir() {
            return root;
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("TDT").join("models");
    }
    PathBuf::from("models")
}

fn model_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.join("models"));
        roots.push(cwd.join("../models"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("models"));
            roots.push(dir.join("../models"));
            roots.push(dir.join("../../models"));
            roots.push(dir.join("../../../models"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("TDT").join("models"));
    }
    roots
}

fn canonicalize_or_clone(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
