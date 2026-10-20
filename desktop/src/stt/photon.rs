use super::models::ModelSpec;
use super::segment;
use super::{LiveAudio, LiveTranscript};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const MAX_CAPTURED_SAMPLES: usize = 120 * 16_000;
/// After a cut, a remainder shorter than 200 ms is the pause itself, not speech.
const MIN_REMAINDER_SAMPLES: usize = 3_200;
const WORKER_SCRIPT: &str = include_str!("photon_worker.py");

const B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        out.push(B64_CHARS[(b0 >> 2) as usize] as char);
        out.push(B64_CHARS[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_CHARS[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_CHARS[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn encode_wav_pcm16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_samples = samples.len() as u32;
    let bytes_per_sample = 2u32;
    let data_size = num_samples * bytes_per_sample;
    let file_size = 36 + data_size;
    let mut wav = Vec::with_capacity((44 + data_size) as usize);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // subchunk size 16
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&1u16.to_le_bytes()); // 1 channel (mono)
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * bytes_per_sample).to_le_bytes()); // byte rate
    wav.extend_from_slice(&bytes_per_sample.to_le_bytes()[..2]); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // 16 bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    for &sample in samples {
        let scaled = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        wav.extend_from_slice(&scaled.to_le_bytes());
    }
    wav
}

#[derive(Debug, Clone)]
enum PythonLauncher {
    Direct(PathBuf),
    Uv(PathBuf),
    Command(String, Vec<String>),
}

/// How `uv run` may use its package cache.
#[derive(Clone, Copy)]
enum CacheUse {
    /// No index request. A start works from the cache alone.
    Offline,
    /// Download the runtime when the cache has nothing, or after a repair.
    Install,
}

impl PythonLauncher {
    /// `Offline` keeps uv on its cache: no index request, so a start needs no
    /// network. Only the first install, or a repair of a broken cache, goes online.
    fn build_command(&self, script_path: &Path, cache: CacheUse) -> Command {
        #[allow(unused_mut)]
        let mut cmd = match self {
            Self::Direct(exe) => {
                let mut c = Command::new(exe);
                c.arg("-u").arg(script_path);
                c
            }
            Self::Uv(uv_exe) => {
                let mut c = Command::new(uv_exe);
                c.arg("run");
                if matches!(cache, CacheUse::Offline) {
                    c.arg("--offline");
                }
                c.arg("--with")
                    .arg("moondream>=2.4.0")
                    .arg("python")
                    .arg("-u")
                    .arg(script_path);
                c
            }
            Self::Command(program, args) => {
                let mut c = Command::new(program);
                for arg in args {
                    c.arg(arg);
                }
                c.arg("-u").arg(script_path);
                c
            }
        };

        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        cmd
    }

    /// Drop Pillow's cached wheel. A later `uv run` downloads it again.
    /// `--reinstall-package` is not enough: uv keeps the broken environment.
    fn cache_clean_pillow(&self) -> Option<Command> {
        let Self::Uv(uv_exe) = self else {
            return None;
        };
        let mut cmd = Command::new(uv_exe);
        cmd.arg("cache").arg("clean").arg("pillow");
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);
        Some(cmd)
    }
}

fn locate_script_file() -> Result<PathBuf, String> {
    let candidate_dir = if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(local).join("TDT")
    } else {
        std::env::temp_dir().join("TDT")
    };

    if !candidate_dir.exists() {
        let _ = fs::create_dir_all(&candidate_dir);
    }

    let script_file = candidate_dir.join("photon_worker.py");
    // Ensure script matches current embedded version
    let needs_write = match fs::read_to_string(&script_file) {
        Ok(existing) => existing != WORKER_SCRIPT,
        Err(_) => true,
    };

    if needs_write {
        let tmp = script_file.with_extension("py.tmp");
        fs::write(&tmp, WORKER_SCRIPT)
            .map_err(|e| format!("Could not write photon_worker.py: {e}"))?;
        fs::rename(&tmp, &script_file)
            .map_err(|e| format!("Could not install photon_worker.py: {e}"))?;
    }

    Ok(script_file)
}

fn test_command_silent(mut cmd: Command) -> bool {
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_python_launcher() -> Result<PythonLauncher, String> {
    static CACHE: std::sync::Mutex<Option<Result<PythonLauncher, String>>> =
        std::sync::Mutex::new(None);
    {
        let cache = CACHE.lock().unwrap();
        if let Some(cached) = &*cache {
            return cached.clone();
        }
    }
    let found = find_python_launcher_uncached();
    if found.is_ok() {
        *CACHE.lock().unwrap() = Some(found.clone());
    }
    found
}

fn find_python_launcher_uncached() -> Result<PythonLauncher, String> {
    // 1. Explicit environment variable
    for env_var in ["VOICE_STT_PHOTON_PYTHON", "VOICE_STT_PYTHON"] {
        if let Some(path_str) = std::env::var_os(env_var) {
            let path = PathBuf::from(path_str);
            if path.is_file() {
                return Ok(PythonLauncher::Direct(path));
            }
            eprintln!("{env_var} is set but {path:?} is not a file; ignoring.");
        }
    }

    // 2. LocalAppData packaged virtualenv
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let venv_py = PathBuf::from(local)
            .join("TDT")
            .join("runtimes")
            .join("photon")
            .join("Scripts")
            .join("python.exe");
        if venv_py.is_file() {
            return Ok(PythonLauncher::Direct(venv_py));
        }
    }

    // 3. Next to executable or models
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let bundled_py = parent
                .join("models")
                .join("photon-runtime")
                .join("Scripts")
                .join("python.exe");
            if bundled_py.is_file() {
                return Ok(PythonLauncher::Direct(bundled_py));
            }
        }
    }

    // 4. UV package manager (supports running without system-wide python packages)
    let uv_candidates = [
        PathBuf::from("uv"),
        // Official installer default: %USERPROFILE%\.local\bin\uv.exe
        directories::BaseDirs::new()
            .map(|b| b.home_dir().join(".local").join("bin").join("uv.exe"))
            .unwrap_or_default(),
        directories::BaseDirs::new()
            .map(|b| b.home_dir().join(".cargo").join("bin").join("uv.exe"))
            .unwrap_or_default(),
        std::env::var_os("LOCALAPPDATA")
            .map(|l| PathBuf::from(l).join("Programs").join("uv").join("uv.exe"))
            .unwrap_or_default(),
    ];

    for candidate in &uv_candidates {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        let mut test_cmd = Command::new(candidate);
        test_cmd.arg("--version");
        if test_command_silent(test_cmd) {
            return Ok(PythonLauncher::Uv(candidate.clone()));
        }
    }

    // 5. System Python with moondream installed
    for py_candidate in ["python", "python3", "py"] {
        let mut test_cmd = Command::new(py_candidate);
        if py_candidate == "py" {
            test_cmd.arg("-3");
        }
        test_cmd.arg("-c").arg("import moondream");
        if test_command_silent(test_cmd) {
            let args = if py_candidate == "py" {
                vec!["-3".to_string()]
            } else {
                Vec::new()
            };
            return Ok(PythonLauncher::Command(py_candidate.to_string(), args));
        }
    }

    Err("Could not find a Python runtime for the speech model. Install uv (powershell -c \"irm https://astral.sh/uv/install.ps1 | iex\") or Python with `pip install moondream`, then restart TDT.".into())
}

struct PhotonWorkerClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// The interpreter the worker runs in, as the worker reports it.
    python: Option<PathBuf>,
}

/// uv resolves its environment on every start, which costs about 0.4 s warm
/// and several seconds after sign-in. The interpreter it chose last time is
/// kept here and started directly.
const REMEMBERED_PYTHON_FILE: &str = "photon-python.txt";

fn remembered_python_in(app_dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(app_dir.join(REMEMBERED_PYTHON_FILE)).ok()?;
    let python = PathBuf::from(text.trim());
    python.is_file().then_some(python)
}

fn remember_python_in(app_dir: &Path, python: &Path) {
    let _ = fs::write(
        app_dir.join(REMEMBERED_PYTHON_FILE),
        python.to_string_lossy().as_bytes(),
    );
}

fn forget_python_in(app_dir: &Path) {
    let _ = fs::remove_file(app_dir.join(REMEMBERED_PYTHON_FILE));
}

enum SpawnError {
    /// The process ended before it answered, such as uv offline with no cached runtime.
    Exited(String),
    /// The worker ran and reported an error; a retry would fail the same way.
    Failed(String),
}

impl SpawnError {
    fn into_message(self) -> String {
        match self {
            Self::Exited(message) | Self::Failed(message) => message,
        }
    }
}

/// An import failure is a broken cached runtime, not a missing speech model.
/// uv will keep serving that cache until the environment is removed.
fn cached_runtime_is_broken(message: &str) -> bool {
    message.contains("cannot import name")
        || message.contains("No module named")
        || message.contains("DLL load failed")
}

/// Directory of the uv archive named in an import error.
/// Only an absolute path under `uv/cache/archive-v0/<id>` is accepted.
fn broken_archive_dir(message: &str) -> Option<PathBuf> {
    let (needle, pos) = ["uv\\cache\\archive-v0\\", "uv/cache/archive-v0/"]
        .into_iter()
        .find_map(|needle| message.find(needle).map(|pos| (needle, pos)))?;
    let path_start = message[..pos]
        .rfind(['(', ' ', '"', '\n', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    let head = &message[path_start..pos];
    if !(head.contains(':') || head.starts_with('/') || head.starts_with('\\')) {
        return None;
    }
    let rest = &message[pos + needle.len()..];
    let id_end = rest.find(['\\', '/', ')', ' ', '"']).unwrap_or(rest.len());
    let id = &rest[..id_end];
    if id.is_empty()
        || id == "."
        || id == ".."
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let mut dir = PathBuf::from(head);
    dir.push("uv");
    dir.push("cache");
    dir.push("archive-v0");
    dir.push(id);
    Some(dir)
}

impl PhotonWorkerClient {
    fn spawn(model_dir: &Path) -> Result<Self, String> {
        let launcher = find_python_launcher()?;
        let script_file = locate_script_file()?;
        let app_dir = match script_file.parent() {
            Some(dir) if matches!(launcher, PythonLauncher::Uv(_)) => dir.to_path_buf(),
            _ => return Self::spawn_with(&launcher, &script_file, model_dir),
        };

        if let Some(python) = remembered_python_in(&app_dir) {
            let direct = PythonLauncher::Direct(python);
            match Self::start(
                direct.build_command(&script_file, CacheUse::Offline),
                model_dir,
            ) {
                Ok(client) => return Ok(client),
                // uv would start the same environment and fail the same way.
                Err(SpawnError::Failed(message)) if !cached_runtime_is_broken(&message) => {
                    return Err(message)
                }
                Err(error) => {
                    eprintln!(
                        "Remembered speech runtime did not start ({}); resolving it with uv.",
                        error.into_message()
                    );
                    forget_python_in(&app_dir);
                }
            }
        }

        let client = Self::spawn_with(&launcher, &script_file, model_dir)?;
        if let Some(python) = &client.python {
            remember_python_in(&app_dir, python);
        }
        Ok(client)
    }

    fn spawn_with(
        launcher: &PythonLauncher,
        script_file: &Path,
        model_dir: &Path,
    ) -> Result<Self, String> {
        let script_file = script_file.to_path_buf();
        if matches!(launcher, PythonLauncher::Uv(_)) {
            match Self::start(
                launcher.build_command(&script_file, CacheUse::Offline),
                model_dir,
            ) {
                Ok(client) => return Ok(client),
                Err(SpawnError::Failed(message)) if cached_runtime_is_broken(&message) => {
                    eprintln!(
                        "Cached speech runtime cannot import ({message}); reinstalling it once."
                    );
                    // uv reuses the environment named in the error, and
                    // `--reinstall-package` links that environment back to the
                    // same incomplete Pillow extract. Remove both, then install.
                    if let Some(dir) = broken_archive_dir(&message) {
                        let _ = fs::remove_dir_all(&dir);
                    }
                    if let Some(mut clean) = launcher.cache_clean_pillow() {
                        let _ = clean.stdout(Stdio::null()).stderr(Stdio::null()).status();
                    }
                    return Self::start(
                        launcher.build_command(&script_file, CacheUse::Install),
                        model_dir,
                    )
                    .map_err(SpawnError::into_message);
                }
                Err(SpawnError::Failed(message)) => return Err(message),
                Err(SpawnError::Exited(message)) => {
                    eprintln!("No cached Photon runtime ({message}); installing it once.");
                }
            }
        }
        Self::start(
            launcher.build_command(&script_file, CacheUse::Install),
            model_dir,
        )
        .map_err(SpawnError::into_message)
    }

    fn start(mut cmd: Command, model_dir: &Path) -> Result<Self, SpawnError> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            SpawnError::Failed(format!("Failed to spawn Photon worker process: {e}"))
        })?;

        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpawnError::Failed(
                "Failed to open pipes for Photon worker".to_string(),
            ));
        };

        let reader = BufReader::new(stdout);
        let mut client = Self {
            child,
            stdin,
            reader,
            python: None,
        };

        // Initialize model in worker
        let init_req = json!({
            "cmd": "init",
            "model_dir": model_dir.to_string_lossy().to_string(),
        });

        // Dropping `client` on an error path kills the child process.
        let resp = client.send_request(&init_req).map_err(SpawnError::Exited)?;
        if resp.get("status").and_then(|s| s.as_str()) != Some("ready") {
            let msg = resp
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown initialization error");
            return Err(SpawnError::Failed(format!(
                "Photon worker initialization failed: {msg}"
            )));
        }
        client.python = resp
            .get("python")
            .and_then(|path| path.as_str())
            .map(PathBuf::from);

        Ok(client)
    }

    fn send_request(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut payload = serde_json::to_string(req).map_err(|e| e.to_string())?;
        payload.push('\n');

        self.stdin
            .write_all(payload.as_bytes())
            .map_err(|e| format!("Failed to write to Photon worker: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("Failed to flush Photon worker stdin: {e}"))?;

        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read response from Photon worker: {e}"))?;

        if line.trim().is_empty() {
            return Err("Photon worker process closed stream unexpectedly".to_string());
        }

        serde_json::from_str(&line)
            .map_err(|e| format!("Invalid JSON from Photon worker: {e} (got {line:?})"))
    }
}

impl Drop for PhotonWorkerClient {
    fn drop(&mut self) {
        // Do not wait for a response; a hung worker must not hang shutdown.
        let payload = b"{\"cmd\": \"shutdown\"}\n";
        let _ = self.stdin.write_all(payload);
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct PhotonParakeetEngine {
    model_dir: PathBuf,
    worker: Arc<Mutex<Option<PhotonWorkerClient>>>,
}

impl PhotonParakeetEngine {
    pub fn new(spec: &ModelSpec, model_dir: &Path) -> Result<Self, String> {
        spec.require_installed(model_dir)?;

        Ok(Self {
            model_dir: model_dir.to_path_buf(),
            worker: Arc::new(Mutex::new(None)),
        })
    }

    pub fn prepare(&self) -> Result<(), String> {
        let mut guard = self.worker.lock();
        if guard.is_none() {
            let client = PhotonWorkerClient::spawn(&self.model_dir)?;
            *guard = Some(client);
        }
        Ok(())
    }

    pub fn transcribe_live_reporting(
        &self,
        rx: &Receiver<LiveAudio>,
    ) -> Result<LiveTranscript, String> {
        self.prepare()?;
        let mut stopped_at = None;
        let mut captured: Vec<f32> = Vec::new();
        // Speech before `committed` is already text in `parts`.
        let mut committed = 0;
        let mut parts: Vec<String> = Vec::new();
        let mut last_any = std::time::Instant::now();

        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(LiveAudio::Chunk(samples)) => {
                    last_any = std::time::Instant::now();
                    if captured.len() < MAX_CAPTURED_SAMPLES {
                        captured.extend_from_slice(&samples);
                        // Transcribe finished sentences while the user still
                        // talks. New chunks wait in the channel meanwhile.
                        if let Some(cut) = segment::pause_cut(&captured, committed) {
                            let text = self.transcribe_samples(&captured[committed..cut])?;
                            if !text.is_empty() {
                                parts.push(text);
                            }
                            committed = cut;
                        }
                    }
                }
                Ok(LiveAudio::Finish(at)) => {
                    stopped_at = Some(at);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // A stalled producer must not hang transcription forever.
                    if last_any.elapsed() > Duration::from_secs(120) {
                        return Err("Audio capture stalled; transcription aborted.".to_string());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        let stopped_at = stopped_at.unwrap_or_else(Instant::now);
        // Duration actually fed to the model, not the full take when the
        // capture cap truncated it.
        let duration_secs = captured.len() as f32 / 16000.0;

        if captured.is_empty() {
            return Ok(LiveTranscript {
                text: String::new(),
                latency_ms: stopped_at.elapsed().as_millis() as u64,
                duration_secs,
            });
        }

        let remainder = &captured[committed..];
        if parts.is_empty() || remainder.len() >= MIN_REMAINDER_SAMPLES {
            let text = self.transcribe_samples(remainder)?;
            if !text.is_empty() {
                parts.push(text);
            }
        }
        let mut text = segment::join_parts(&parts);

        if text.is_empty() {
            if let Some(boosted) = super::engine::boost_quiet_speech(&captured) {
                if let Ok(boosted_text) = self.transcribe_samples(&boosted) {
                    text = boosted_text;
                }
            }
        }

        Ok(LiveTranscript {
            text,
            latency_ms: stopped_at.elapsed().as_millis() as u64,
            duration_secs,
        })
    }

    /// One request to the worker. A dead worker is dropped so the next take respawns it.
    fn transcribe_samples(&self, samples: &[f32]) -> Result<String, String> {
        let req = json!({
            "cmd": "transcribe",
            "audio": b64_encode(&encode_wav_pcm16(samples, 16000)),
        });

        let mut guard = self.worker.lock();
        let client = guard
            .as_mut()
            .ok_or_else(|| "Photon worker not running".to_string())?;

        let resp = match client.send_request(&req) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Photon worker died mid-transcription: {e}");
                *guard = None;
                return Err(format!("Photon transcription failed: {e}"));
            }
        };

        if resp.get("status").and_then(|s| s.as_str()) != Some("ok") {
            let msg = resp
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Transcription error");
            return Err(format!("Photon worker error: {msg}"));
        }

        Ok(resp
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string())
    }

    pub fn release(&self) {
        *self.worker.lock() = None;
    }

    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.worker.lock().is_some()
    }
}

pub fn pcm16_wav(path: &std::path::Path) -> Option<Vec<f32>> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut channels = 1u16;
    let mut rate = 0u32;
    let mut bits = 16u16;
    let mut data: Option<&[u8]> = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let body = offset + 8;
        if body + size > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().ok()?);
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().ok()?);
        } else if id == b"data" {
            data = Some(&bytes[body..body + size]);
        }
        offset = body + size + (size % 2);
    }
    let data = data?;
    if bits != 16 || channels == 0 || rate != 16_000 {
        return None;
    }
    let width = channels as usize;
    let mut mono = Vec::with_capacity(data.len() / (2 * width));
    for frame in data.chunks_exact(2 * width) {
        let mut sum = 0.0f32;
        for channel in 0..width {
            let sample = i16::from_le_bytes([frame[channel * 2], frame[channel * 2 + 1]]);
            sum += f32::from(sample) / f32::from(i16::MAX);
        }
        mono.push(sum / width as f32);
    }
    Some(mono)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::models::{by_id, DEFAULT_MODEL_ID};
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn command_args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("voice-stt-photon-test-{tag}-{unique}"))
    }

    #[test]
    fn b64_encode_matches_standard_rfc4648() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encode_wav_pcm16_produces_valid_riff_header() {
        let samples = vec![0.0f32; 1600];
        let wav = encode_wav_pcm16(&samples, 16000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len, 3200);
        assert_eq!(wav.len(), 44 + 3200);
    }

    #[test]
    fn the_resolved_python_is_remembered_until_it_is_gone() {
        let dir = unique_dir("remembered-python");
        fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(remembered_python_in(&dir), None);

        let python = dir.join("python.exe");
        fs::write(&python, b"stub").expect("stub python");
        remember_python_in(&dir, &python);
        assert_eq!(remembered_python_in(&dir), Some(python.clone()));

        fs::remove_file(&python).expect("remove stub");
        assert_eq!(remembered_python_in(&dir), None);

        fs::write(&python, b"stub").expect("stub python");
        remember_python_in(&dir, &python);
        forget_python_in(&dir);
        assert_eq!(remembered_python_in(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_fails_cleanly_without_a_model() {
        let missing = unique_dir("redux-missing").join("no-such-dir");
        assert!(
            PhotonWorkerClient::spawn(&missing).is_err(),
            "spawn should fail without model files"
        );
    }

    #[test]
    fn constructing_photon_engine_does_not_load_model() {
        let spec = by_id(DEFAULT_MODEL_ID).expect("redux spec");
        let model_dir = unique_dir("redux-mock");
        fs::create_dir_all(&model_dir).expect("temp dir");
        for file in spec.files {
            fs::write(model_dir.join(file.name), b"mock-data").expect("mock file");
        }

        let engine = PhotonParakeetEngine::new(spec, &model_dir)
            .expect("construction should validate without launching python");
        assert!(
            !engine.is_loaded(),
            "worker must not be started during construction"
        );

        fs::remove_dir_all(model_dir).expect("cleanup");
    }

    #[test]
    fn broken_pillow_cache_is_reinstalled_once() {
        // The message the worker returns when the cached Pillow wheel has no Image.py.
        assert!(cached_runtime_is_broken(
            "cannot import name 'Image' from 'PIL' (C:\\Users\\hi\\AppData\\Local\\uv\\cache\\archive-v0\\dJP2kpEhvs5M73D9\\Lib\\site-packages\\PIL\\__init__.py)"
        ));
        assert!(!cached_runtime_is_broken(
            "Model directory is missing or not a directory: \"C:\\missing\""
        ));
    }

    #[test]
    fn repair_removes_only_the_named_uv_archive_and_installs_online() {
        let message = "cannot import name 'Image' from 'PIL' (C:\\Users\\hi\\AppData\\Local\\uv\\cache\\archive-v0\\dJP2kpEhvs5M73D9\\Lib\\site-packages\\PIL\\__init__.py)";
        assert_eq!(
            broken_archive_dir(message),
            Some(PathBuf::from(
                "C:\\Users\\hi\\AppData\\Local\\uv\\cache\\archive-v0\\dJP2kpEhvs5M73D9"
            ))
        );
        assert_eq!(
            broken_archive_dir(
                "cannot import name 'Image' from 'PIL' (C:\\Users\\hi\\AppData\\Local\\uv\\cache\\archive-v0\\f-Vd1EQPV2KNTyWi\\Lib\\site-packages\\PIL\\__init__.py)"
            ),
            Some(PathBuf::from(
                "C:\\Users\\hi\\AppData\\Local\\uv\\cache\\archive-v0\\f-Vd1EQPV2KNTyWi"
            ))
        );
        assert!(broken_archive_dir(
            "cannot import name 'Image' from 'PIL' (C:\\Windows\\System32\\archive-v0\\dJP2kpEhvs5M73D9\\PIL\\__init__.py)"
        )
        .is_none());
        assert!(broken_archive_dir(
            "Model directory is missing or not a directory: \"C:\\missing\""
        )
        .is_none());

        let launcher = PythonLauncher::Uv("uv".into());
        let script = PathBuf::from("photon_worker.py");
        assert_eq!(
            command_args(&launcher.cache_clean_pillow().expect("uv clean command")),
            vec!["cache", "clean", "pillow"]
        );
        assert_eq!(
            command_args(&launcher.build_command(&script, CacheUse::Install)),
            vec![
                "run",
                "--with",
                "moondream>=2.4.0",
                "python",
                "-u",
                "photon_worker.py",
            ]
        );
        assert_eq!(
            command_args(&launcher.build_command(&script, CacheUse::Offline)),
            vec![
                "run",
                "--offline",
                "--with",
                "moondream>=2.4.0",
                "python",
                "-u",
                "photon_worker.py",
            ]
        );
    }
}
