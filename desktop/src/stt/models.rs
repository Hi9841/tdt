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
    #[allow(dead_code)]
    pub blurb: &'static str,
    pub size_label: &'static str,
    pub size_bytes: u64,
    pub hf_repo: &'static str,
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

/// Recommended default: FluidAudio's English Parakeet Unified 0.6B, INT8 streaming.
pub const DEFAULT_MODEL_ID: &str = "parakeet-unified-en-0.6b-int8";

/// Existing installs stored this id for the same FluidAudio Unified package.
pub const PARAKEET_Q8_ID: &str = "parakeet-unified-en-0.6b-q8";

/// Shared on-disk folder. INT8 and Q8 are the same sherpa-onnx streaming export.
const UNIFIED_DIR: &str = "parakeet-unified-en-0.6b-q8";

/// FluidAudio Windows path: sherpa-onnx INT8 streaming export of
/// nvidia/parakeet-unified-en-0.6b (1120 ms). One download covers both chips.
const UNIFIED_REPO: &str =
    "csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-1120ms";

const UNIFIED_FILES: &[ModelFile] = &[
    ModelFile {
        name: "encoder.int8.onnx",
        sha256: "1C03F1192DE41771384AF22972CA10203613BA56197A024F275B86727CD35911",
    },
    ModelFile {
        name: "decoder.int8.onnx",
        sha256: "34FEA72425D2506600772BA191A6D3F99C0710ABDB68D9A3DC89FA8CB2AA473A",
    },
    ModelFile {
        name: "joiner.int8.onnx",
        sha256: "869F43F7D24595C55581AD3BF249A935FB8A71389FBDAA7504B9F46F93140F8A",
    },
    ModelFile {
        name: "tokens.txt",
        sha256: "DC0B4584AB2E4DDBF888425C076C61B736E7356A015250DB7D307E6F1A8188FF",
    },
];

/// Retired ids kept for migration only; they resolve to Parakeet INT8.
fn migrated_id(_id: &str) -> &'static str {
    DEFAULT_MODEL_ID
}

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: DEFAULT_MODEL_ID,
        label: "Parakeet INT8",
        blurb: "FluidAudio English Unified. INT8 streaming. Recommended.",
        size_label: "632 MB",
        size_bytes: 663_048_980,
        hf_repo: UNIFIED_REPO,
        dir_name: UNIFIED_DIR,
        files: UNIFIED_FILES,
    },
    ModelSpec {
        id: PARAKEET_Q8_ID,
        label: "Parakeet Q8",
        blurb: "FluidAudio English Unified. Same 8-bit streaming weights as INT8.",
        size_label: "632 MB",
        size_bytes: 663_048_980,
        hf_repo: UNIFIED_REPO,
        dir_name: UNIFIED_DIR,
        files: UNIFIED_FILES,
    },
];

pub const DEFAULT: &ModelSpec = &CATALOG[0];

pub fn by_id(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|spec| spec.id == id.trim())
}

pub fn resolve(id: &str) -> &'static ModelSpec {
    if let Some(spec) = by_id(id) {
        return spec;
    }
    by_id(migrated_id(id)).unwrap_or(DEFAULT)
}

impl ModelSpec {
    pub fn dir_in(&self, root: &Path) -> PathBuf {
        root.join(self.dir_name)
    }

    pub fn is_installed_in(&self, dir: &Path) -> bool {
        self.files.iter().all(|file| dir.join(file.name).is_file())
    }

    pub fn tokens_path(&self, dir: &Path) -> PathBuf {
        let name = self
            .files
            .iter()
            .find(|file| file.name.contains("tokens"))
            .map(|file| file.name)
            .unwrap_or("tokens.txt");
        dir.join(name)
    }

    pub fn transducer_part(&self, dir: &Path, kind: &str) -> PathBuf {
        let name = self
            .files
            .iter()
            .find(|file| file.name.starts_with(kind))
            .map(|file| file.name)
            .unwrap_or(match kind {
                "encoder" => "encoder.int8.onnx",
                "decoder" => "decoder.int8.onnx",
                _ => "joiner.int8.onnx",
            });
        dir.join(name)
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
        .trim_end_matches(".onnx")
        .trim_end_matches(".txt")
        .replace(".int8", "")
        .to_string()
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
        on_progress(DownloadProgress {
            done,
            total,
            file: file.name.to_string(),
            file_index,
            file_count,
        });
        let dest = dest_dir.join(file.name);
        if dest.is_file()
            && (file.sha256.is_empty() || file_sha256(&dest)?.eq_ignore_ascii_case(file.sha256))
        {
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
            "{HF_BASE}/{}/resolve/main/{}?download=true",
            spec.hf_repo, file.name
        );
        download_file(&url, &dest, file.sha256, |chunk| {
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
    let last = spec.files.last().map(|file| file.name).unwrap_or("");
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
    mut on_chunk: impl FnMut(u64),
) -> Result<(), String> {
    let tmp = dest.with_file_name(format!(
        "{}.download",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }

    let response = download_agent()
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
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn download_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(7200))
        .timeout_write(Duration::from_secs(60))
        .user_agent(&format!(
            "TDT/{} (+https://github.com/Hi9841/tdt)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
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

#[cfg(test)]
mod tests {
    use super::{
        by_id, file_label, format_mb, percent, resolve, ModelSpec, CATALOG, DEFAULT,
        DEFAULT_MODEL_ID,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("tdt-model-{tag}-{unique}"))
    }

    #[test]
    fn catalog_ids_are_unique_and_complete() {
        let mut ids = Vec::new();
        for spec in CATALOG {
            assert!(!spec.id.is_empty());
            assert!(spec.size_bytes > 0);
            assert!(spec.files.iter().any(|file| file.name.contains("tokens")));
            for part in ["encoder", "decoder", "joiner"] {
                assert!(
                    spec.files.iter().any(|file| file.name.starts_with(part)),
                    "parakeet {part} missing",
                );
            }
            assert!(!ids.contains(&spec.id), "duplicate model id {}", spec.id);
            ids.push(spec.id);
        }
        assert!(ids.contains(&DEFAULT_MODEL_ID));
        assert!(ids.contains(&super::PARAKEET_Q8_ID));
        assert_eq!(
            CATALOG.len(),
            2,
            "catalog keeps Parakeet INT8 and Parakeet Q8",
        );
        assert_eq!(
            by_id(DEFAULT_MODEL_ID).map(|spec| spec.id),
            Some(DEFAULT_MODEL_ID)
        );
    }

    #[test]
    fn default_is_parakeet_int8_with_q8_alias() {
        assert_eq!(resolve("").id, DEFAULT_MODEL_ID);
        assert_eq!(DEFAULT_MODEL_ID, "parakeet-unified-en-0.6b-int8");
        assert_eq!(DEFAULT.id, DEFAULT_MODEL_ID);
        assert_eq!(
            CATALOG.iter().map(|spec| spec.label).collect::<Vec<_>>(),
            vec!["Parakeet INT8", "Parakeet Q8"]
        );
        let int8 = by_id(DEFAULT_MODEL_ID).expect("int8");
        let q8 = by_id(super::PARAKEET_Q8_ID).expect("q8");
        assert_eq!(int8.dir_name, q8.dir_name);
        assert_eq!(int8.hf_repo, q8.hf_repo);
        assert_eq!(
            int8.files.iter().map(|file| file.name).collect::<Vec<_>>(),
            q8.files.iter().map(|file| file.name).collect::<Vec<_>>()
        );
        assert!(int8
            .files
            .iter()
            .all(|file| file.name.contains("int8") || file.name.contains("tokens")));
    }

    #[test]
    fn retired_ids_migrate_to_parakeet_int8() {
        for id in [
            "whisper-small",
            "whisper-medium",
            "sensevoice-small",
            "sensevoice-full",
            "moonshine-medium-streaming",
            "parakeet-tdt-0.6b-v3",
        ] {
            assert_eq!(resolve(id).id, DEFAULT_MODEL_ID, "{id}");
        }
        assert_eq!(resolve(super::PARAKEET_Q8_ID).id, super::PARAKEET_Q8_ID);
    }

    #[test]
    fn unknown_id_falls_back_to_parakeet_int8() {
        assert_eq!(resolve("nope").id, DEFAULT_MODEL_ID);
        assert_eq!(
            by_id(DEFAULT_MODEL_ID).map(|spec| spec.id),
            Some(DEFAULT_MODEL_ID)
        );
    }

    #[test]
    fn install_check_requires_every_catalog_file() {
        let spec: &ModelSpec =
            by_id(DEFAULT_MODEL_ID).expect("parakeet q8 default is in the catalog");
        let dir = unique_dir("install");
        fs::create_dir_all(&dir).expect("temp dir");
        assert!(!spec.is_installed_in(&dir));
        for file in spec.files {
            fs::write(dir.join(file.name), b"placeholder").expect("placeholder");
        }
        assert!(spec.is_installed_in(&dir));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn format_mb_rounds_known_sizes() {
        assert_eq!(format_mb(670_478_772), "639 MB");
        assert_eq!(format_mb(663_048_980), "632 MB");
        assert_eq!(format_mb(946_072_270), "902 MB");
    }

    #[test]
    fn percent_hits_zero_and_one_hundred() {
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(100, 100), 100);
        assert_eq!(percent(1, 0), 0);
    }

    #[test]
    fn file_label_drops_weight_suffixes() {
        assert_eq!(file_label("medium-decoder.int8.onnx"), "medium-decoder");
        assert_eq!(file_label("tokens.txt"), "tokens");
        assert_eq!(file_label("model.onnx"), "model");
    }
}
