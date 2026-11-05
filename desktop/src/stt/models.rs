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
    /// How Trit holds the weights: the fastest choice that keeps the
    /// model's accuracy (see the model's notes).
    pub precision: trit::Precision,
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

/// NVIDIA Parakeet Unified English 0.6B (RNNT) in ggml's Q8_0, the file
/// that Hex ships (handy-computer, pinned below). Trit reads the GGUF file
/// and holds it as int8: on the 456 English test clips (Trit, CPU) 177 word
/// errors (Ultra 205, Redux 301) at 40 ms a clip (Ultra 36 ms); with the
/// stored Q8_0 kernel 178 errors at 52 ms. 731 MB on disk. No word comes out
/// of silence or noise. It replaces Parakeet Ultra.
pub const UNIFIED_ID: &str = "parakeet-unified-en";

const UNIFIED_FILES: &[ModelFile] = &[ModelFile {
    name: "parakeet-unified-en-0.6b-Q8_0.gguf",
    sha256: "4B50B6DD862BF6E346929AAF4F5EAACEC003BFA3F56462D6C874B41EF2F38795",
}];

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
        precision: trit::Precision::Auto,
    },
    ModelSpec {
        id: UNIFIED_ID,
        label: "Parakeet Unified English",
        size_label: "731 MB",
        size_bytes: 731_357_568,
        hf_repo: "handy-computer/parakeet-unified-en-0.6b-gguf",
        revision: "7e948f21b7bdbac698d3318db9d350f1096f3b6c",
        // Not the name of the retired ONNX folders, which hold a config.json.
        dir_name: "parakeet-unified-en-gguf",
        files: UNIFIED_FILES,
        // Auto would run the stored Q8_0; Trit's own int8 is faster (40 ms
        // against 52 ms a clip) with one error fewer on the test clips.
        precision: trit::Precision::Int8,
    },
];

/// Ids of models TDT no longer offers, and the model that replaces each.
/// Any other unknown id resolves to `DEFAULT`.
const REPLACED: &[(&str, &str)] = &[("parakeet-ultra", UNIFIED_ID)];

pub const DEFAULT: &ModelSpec = &CATALOG[0];

pub fn by_id(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|spec| spec.id == id.trim())
}

/// A known id, else its replacement (Parakeet Ultra: Unified English), else
/// Parakeet Redux (the retired INT8, Q8, Whisper, Moonshine, SenseVoice, and
/// Parakeet v3 ids).
pub fn resolve(id: &str) -> &'static ModelSpec {
    by_id(id)
        .or_else(|| {
            REPLACED
                .iter()
                .find(|(old, _)| *old == id.trim())
                .and_then(|(_, new)| by_id(new))
        })
        .unwrap_or(DEFAULT)
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

/// A try that failed and added no bytes. This many in sequence stop the download.
const DOWNLOAD_TRIES: u32 = 3;

fn download_file(
    url: &str,
    dest: &Path,
    expected_sha: &str,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let partial = dest.with_file_name(format!(
        "{}.download",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    // The bytes of this file that `on_chunk` has had.
    let mut reported = 0u64;
    let mut failed = 0;
    loop {
        let before = file_len(&partial);
        let error =
            match download_file_once(url, dest, &partial, expected_sha, &mut reported, on_chunk) {
                Ok(()) => return Ok(()),
                Err(error) => error,
            };
        eprintln!("Model download try failed: {error}");
        // A try that added bytes is progress: the next one continues from there.
        failed = if file_len(&partial) > before {
            0
        } else {
            failed + 1
        };
        if failed >= DOWNLOAD_TRIES {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(500 * u64::from(failed + 1)));
    }
}

/// One request. It continues the partial file of an earlier try when the
/// server agrees, and starts at zero when not. A partial file with other
/// content fails the checksum, and the next try then starts at zero.
fn download_file_once(
    url: &str,
    dest: &Path,
    partial: &Path,
    expected_sha: &str,
    reported: &mut u64,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let have = file_len(partial);
    let mut request = model_download_agent()
        .get(url)
        .set("Accept", "application/octet-stream");
    if have > 0 {
        request = request.set("Range", &format!("bytes={have}-"));
    }
    let response = match request.call() {
        Ok(response) => response,
        // The partial file is as long as the file, or longer.
        Err(ureq::Error::Status(416, _)) => {
            let _ = fs::remove_file(partial);
            return Err("Download failed: the server could not continue the file".to_string());
        }
        Err(error) => return Err(format!("Download failed: {error}")),
    };
    let continues = have > 0
        && response.status() == 206
        && response
            .header("Content-Range")
            .is_some_and(|range| range.trim().starts_with(&format!("bytes {have}-")));

    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut file = if continues {
        hash_file_into(partial, &mut hasher)?;
        written = have;
        fs::OpenOptions::new().append(true).open(partial)
    } else {
        File::create(partial)
    }
    .map_err(|error| format!("Could not write model file: {error}"))?;
    // After a start at zero, the bytes of the earlier try are not reported again.
    let mut report = |written: u64, reported: &mut u64| {
        if written > *reported {
            on_chunk(written - *reported);
            *reported = written;
        }
    };
    report(written, &mut *reported);

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
        written += read as u64;
        report(written, &mut *reported);
    }
    drop(file);

    let actual = format!("{:X}", hasher.finalize());
    if !expected_sha.is_empty() && !actual.eq_ignore_ascii_case(expected_sha) {
        let _ = fs::remove_file(partial);
        return Err(format!(
            "SHA256 mismatch for {}",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
    }

    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(partial, dest).map_err(|error| format!("Could not finalize model file: {error}"))?;
    Ok(())
}

fn hash_file_into(path: &Path, hasher: &mut Sha256) -> Result<(), String> {
    let mut file =
        File::open(path).map_err(|error| format!("Could not read model file: {error}"))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|error| format!("Could not read model file: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buf[..read]);
    }
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hash_file_into(path, &mut hasher)?;
    Ok(format!("{:X}", hasher.finalize()))
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |meta| meta.len())
}

/// The HTTP agent for model downloads. The read limit is for one read, not
/// for the file: a connection that gives no bytes for this long fails, and
/// the next try continues the file.
#[doc(hidden)]
pub fn download_agent(user_agent: &str) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
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
