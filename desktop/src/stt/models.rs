use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const HF_BASE: &str = "https://huggingface.co";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    SenseVoice,
    Whisper,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub chip: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    pub size_label: &'static str,
    pub size_bytes: u64,
    pub family: ModelFamily,
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

pub const DEFAULT_MODEL_ID: &str = "sensevoice-small";

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: DEFAULT_MODEL_ID,
        chip: "Small",
        label: "SenseVoice Small",
        blurb: "Fast multilingual, loaded while you talk",
        size_label: "228 MB",
        size_bytes: 239_549_735,
        family: ModelFamily::SenseVoice,
        hf_repo: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
        dir_name: "sensevoice",
        files: &[
            ModelFile {
                name: "model.int8.onnx",
                sha256: "C71F0CE00BEC95B07744E116345E33D8CBBE08CEF896382CF907BF4B51A2CD51",
            },
            ModelFile {
                name: "tokens.txt",
                sha256: "F449EB28DC567533D7FA59BE34E2ABCA8784F771850C78A47FB731A31429A1DC",
            },
        ],
    },
    ModelSpec {
        id: "sensevoice-full",
        chip: "Full",
        label: "SenseVoice Full",
        blurb: "Same languages, higher quality, slower",
        size_label: "894 MB",
        size_bytes: 937_933_072,
        family: ModelFamily::SenseVoice,
        hf_repo: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
        dir_name: "sensevoice",
        files: &[
            ModelFile {
                name: "model.onnx",
                sha256: "977016BD9C79F9EB343430B5CC305E07AB64D5212DFF41B0DCFA1694BEE9A8CB",
            },
            ModelFile {
                name: "tokens.txt",
                sha256: "F449EB28DC567533D7FA59BE34E2ABCA8784F771850C78A47FB731A31429A1DC",
            },
        ],
    },
    ModelSpec {
        id: "whisper-small",
        chip: "Whisper",
        label: "Whisper Small",
        blurb: "Many languages, loaded while you talk",
        size_label: "358 MB",
        size_bytes: 375_485_327,
        family: ModelFamily::Whisper,
        hf_repo: "csukuangfj/sherpa-onnx-whisper-small",
        dir_name: "whisper-small",
        files: &[
            ModelFile {
                name: "small-encoder.int8.onnx",
                sha256: "4CBE7B22FA9026B843B60A68640C747DE05BAFB1A11B57EDC0E66C232D9F33A9",
            },
            ModelFile {
                name: "small-decoder.int8.onnx",
                sha256: "ACAD50B5C782696E91B55914CC5AB4F756F1532F76E22AA6FC615F39FB69A8EE",
            },
            ModelFile {
                name: "small-tokens.txt",
                sha256: "B34B360DBB493E781E479794586D661700670D65564001F23024971D1F2FA126",
            },
        ],
    },
    ModelSpec {
        id: "whisper-medium",
        chip: "Medium",
        label: "Whisper Medium",
        blurb: "Higher quality Whisper, slower, 902 MB",
        size_label: "902 MB",
        size_bytes: 946_072_270,
        family: ModelFamily::Whisper,
        hf_repo: "csukuangfj/sherpa-onnx-whisper-medium",
        dir_name: "whisper-medium",
        files: &[
            ModelFile {
                name: "medium-encoder.int8.onnx",
                sha256: "1C54582B4D829DE0089F6CB63BBBDB3BF7555398BACAF855FBECF1A84DFD193E",
            },
            ModelFile {
                name: "medium-decoder.int8.onnx",
                sha256: "595D00A338A365A7BFA0CA7F296CABC639583BEF770AB6130DF90F49A6412747",
            },
            ModelFile {
                name: "medium-tokens.txt",
                sha256: "B34B360DBB493E781E479794586D661700670D65564001F23024971D1F2FA126",
            },
        ],
    },
];

pub const DEFAULT: &ModelSpec = &CATALOG[0];

pub fn by_id(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|spec| spec.id == id)
}

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

    pub fn tokens_path(&self, dir: &Path) -> PathBuf {
        let name = self
            .files
            .iter()
            .find(|file| file.name.contains("tokens"))
            .map(|file| file.name)
            .unwrap_or("tokens.txt");
        dir.join(name)
    }

    pub fn sense_voice_model(&self, dir: &Path) -> PathBuf {
        let name = self
            .files
            .iter()
            .find(|file| file.name.ends_with(".onnx"))
            .map(|file| file.name)
            .unwrap_or("model.int8.onnx");
        dir.join(name)
    }

    pub fn whisper_encoder(&self, dir: &Path) -> PathBuf {
        let name = self
            .files
            .iter()
            .find(|file| file.name.contains("encoder"))
            .map(|file| file.name)
            .unwrap_or("encoder.onnx");
        dir.join(name)
    }

    pub fn whisper_decoder(&self, dir: &Path) -> PathBuf {
        let name = self
            .files
            .iter()
            .find(|file| file.name.contains("decoder"))
            .map(|file| file.name)
            .unwrap_or("decoder.onnx");
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

pub fn whisper_language(language: &str) -> Option<String> {
    match language.trim() {
        "" | "auto" => None,
        other => Some(other.to_string()),
    }
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
    if !actual.eq_ignore_ascii_case(expected_sha) {
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
        by_id, file_label, format_mb, percent, resolve, whisper_language, ModelFamily, ModelSpec,
        CATALOG, DEFAULT_MODEL_ID,
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
            assert!(!spec.chip.is_empty());
            assert!(spec.size_bytes > 0);
            assert!(spec.files.iter().any(|file| file.name.contains("tokens")));
            match spec.family {
                ModelFamily::SenseVoice => {
                    assert!(spec.files.iter().any(|file| file.name.ends_with(".onnx")));
                }
                ModelFamily::Whisper => {
                    assert!(spec.files.iter().any(|file| file.name.contains("encoder")));
                    assert!(spec.files.iter().any(|file| file.name.contains("decoder")));
                }
            }
            assert!(!ids.contains(&spec.id), "duplicate model id {}", spec.id);
            ids.push(spec.id);
        }
        assert!(ids.contains(&DEFAULT_MODEL_ID));
    }

    #[test]
    fn unknown_id_falls_back_to_sensevoice_small() {
        assert_eq!(resolve("nope").id, DEFAULT_MODEL_ID);
        assert_eq!(
            by_id(DEFAULT_MODEL_ID).map(|spec| spec.id),
            Some(DEFAULT_MODEL_ID)
        );
    }

    #[test]
    fn whisper_auto_language_is_empty() {
        assert_eq!(whisper_language("auto"), None);
        assert_eq!(whisper_language(""), None);
        assert_eq!(whisper_language("en").as_deref(), Some("en"));
        assert_eq!(whisper_language("yue").as_deref(), Some("yue"));
    }

    #[test]
    fn install_check_requires_every_catalog_file() {
        let spec: &ModelSpec = by_id("whisper-small").expect("whisper-small is in the catalog");
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
        assert_eq!(format_mb(239_549_735), "228 MB");
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
