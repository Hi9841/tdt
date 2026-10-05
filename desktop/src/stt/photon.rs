use super::models::ModelSpec;
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

impl PythonLauncher {
    fn build_command(&self, script_path: &Path) -> Command {
        #[allow(unused_mut)]
        let mut cmd = match self {
            Self::Direct(exe) => {
                let mut c = Command::new(exe);
                c.arg("-u").arg(script_path);
                c
            }
            Self::Uv(uv_exe) => {
                let mut c = Command::new(uv_exe);
                c.arg("run")
                    .arg("--with")
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
        fs::write(&script_file, WORKER_SCRIPT)
            .map_err(|e| format!("Could not write photon_worker.py: {e}"))?;
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
    // 1. Explicit environment variable
    for env_var in ["VOICE_STT_PHOTON_PYTHON", "VOICE_STT_PYTHON"] {
        if let Some(path_str) = std::env::var_os(env_var) {
            let path = PathBuf::from(path_str);
            if path.is_file() {
                return Ok(PythonLauncher::Direct(path));
            }
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
}

impl PhotonWorkerClient {
    fn spawn(model_dir: &Path) -> Result<Self, String> {
        let launcher = find_python_launcher()?;
        let script_file = locate_script_file()?;

        let mut cmd = launcher.build_command(&script_file);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn Photon worker process: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open stdin for Photon worker".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open stdout for Photon worker".to_string())?;

        let reader = BufReader::new(stdout);
        let mut client = Self {
            child,
            stdin,
            reader,
        };

        // Initialize model in worker
        let init_req = json!({
            "cmd": "init",
            "model_dir": model_dir.to_string_lossy().to_string(),
        });

        let resp = client.send_request(&init_req)?;
        if resp.get("status").and_then(|s| s.as_str()) != Some("ready") {
            let msg = resp
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown initialization error");
            return Err(format!("Photon worker initialization failed: {msg}"));
        }

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
        let _ = self.send_request(&json!({"cmd": "shutdown"}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct PhotonParakeetEngine {
    model_dir: PathBuf,
    label: String,
    current_language: Arc<Mutex<String>>,
    worker: Arc<Mutex<Option<PhotonWorkerClient>>>,
}

impl PhotonParakeetEngine {
    pub fn new(spec: &ModelSpec, model_dir: &Path, language: &str) -> Result<Self, String> {
        spec.require_installed(model_dir)?;
        let lang_str = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };

        Ok(Self {
            model_dir: model_dir.to_path_buf(),
            label: spec.label.to_string(),
            current_language: Arc::new(Mutex::new(lang_str)),
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
        _partials: Option<&Mutex<String>>,
    ) -> Result<LiveTranscript, String> {
        self.prepare()?;
        let mut samples_seen = 0usize;
        let mut stopped_at = None;
        let mut captured: Vec<f32> = Vec::new();

        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(LiveAudio::Chunk(samples)) => {
                    samples_seen += samples.len();
                    if captured.len() < MAX_CAPTURED_SAMPLES {
                        captured.extend_from_slice(&samples);
                    }
                }
                Ok(LiveAudio::Finish(at)) => {
                    stopped_at = Some(at);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        let stopped_at = stopped_at.unwrap_or_else(Instant::now);
        let duration_secs = samples_seen as f32 / 16000.0;

        if captured.is_empty() {
            return Ok(LiveTranscript {
                text: String::new(),
                latency_ms: stopped_at.elapsed().as_millis() as u64,
                duration_secs,
            });
        }

        let wav_bytes = encode_wav_pcm16(&captured, 16000);
        let audio_b64 = b64_encode(&wav_bytes);

        let req = json!({
            "cmd": "transcribe",
            "audio": audio_b64,
        });

        let mut guard = self.worker.lock();
        let client = guard
            .as_mut()
            .ok_or_else(|| "Photon worker not running".to_string())?;

        let resp = match client.send_request(&req) {
            Ok(r) => r,
            Err(e) => {
                // If worker connection severed, clear dead client
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

        let mut text = resp
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if text.is_empty() {
            if let Some(boosted) = super::engine::boost_quiet_speech(&captured) {
                let boosted_wav = encode_wav_pcm16(&boosted, 16000);
                let boosted_b64 = b64_encode(&boosted_wav);
                let boosted_req = json!({
                    "cmd": "transcribe",
                    "audio": boosted_b64,
                });
                if let Ok(boosted_resp) = client.send_request(&boosted_req) {
                    if boosted_resp.get("status").and_then(|s| s.as_str()) == Some("ok") {
                        let boosted_text = boosted_resp
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !boosted_text.is_empty() {
                            text = boosted_text;
                        }
                    }
                }
            }
        }

        Ok(LiveTranscript {
            text,
            latency_ms: stopped_at.elapsed().as_millis() as u64,
            duration_secs,
        })
    }

    pub fn release(&self) {
        *self.worker.lock() = None;
    }

    pub fn set_language(&self, language: &str) -> Result<(), String> {
        let language = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };
        let mut current_lang = self.current_language.lock();
        *current_lang = language.clone();
        drop(current_lang);

        self.release();
        println!("{} language set to: {}", self.label, language);
        Ok(())
    }

    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.worker.lock().is_some()
    }
}

#[allow(dead_code)]
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
    use crate::stt::models::{by_id, PARAKEET_REDUX_ID};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn constructing_photon_engine_does_not_load_model() {
        let spec = by_id(PARAKEET_REDUX_ID).expect("redux spec");
        let model_dir = unique_dir("redux-mock");
        fs::create_dir_all(&model_dir).expect("temp dir");
        for file in spec.files {
            fs::write(model_dir.join(file.name), b"mock-data").expect("mock file");
        }

        let engine = PhotonParakeetEngine::new(spec, &model_dir, "en")
            .expect("construction should validate without launching python");
        assert!(
            !engine.is_loaded(),
            "worker must not be started during construction"
        );

        fs::remove_dir_all(model_dir).expect("cleanup");
    }
}
