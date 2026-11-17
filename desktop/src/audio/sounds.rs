use std::f32::consts::PI;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub enum SoundEffect {
    StartListening,
    StopListening,
    Success,
    Error,
}

static START_WAV: OnceLock<Vec<u8>> = OnceLock::new();
static STOP_WAV: OnceLock<Vec<u8>> = OnceLock::new();
static SUCCESS_WAV: OnceLock<Vec<u8>> = OnceLock::new();
static ERROR_WAV: OnceLock<Vec<u8>> = OnceLock::new();

impl SoundEffect {
    /// Every cue, for `prewarm`.
    const ALL: [SoundEffect; 4] = [
        SoundEffect::StartListening,
        SoundEffect::StopListening,
        SoundEffect::Success,
        SoundEffect::Error,
    ];
}

/// Build every cue once so the first hotkey does not synthesize on the UI path.
pub fn prewarm() {
    for effect in SoundEffect::ALL {
        let _ = wav_for(effect);
    }
}

pub fn play_sound(effect: SoundEffect) {
    let wav_bytes: &'static [u8] = wav_for(effect);

    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    // SAFETY: `wav_bytes` is `'static`. With SND_MEMORY, PlaySoundW reads the WAV from that address and
    // never as text, and SND_ASYNC needs it to stay valid, which `'static` guarantees.
    unsafe {
        // SND_ASYNC replaces any in-flight sound: a success chime cuts off
        // the start blip. That overlap is accepted UI behavior, not a bug.
        // The PCWSTR cast is only an address under SND_MEMORY; the WAV
        // bytes are never read as UTF-16.
        let _ = PlaySoundW(
            PCWSTR(wav_bytes.as_ptr() as *const u16),
            None,
            SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
        );
    }
}

fn wav_for(effect: SoundEffect) -> &'static [u8] {
    match effect {
        SoundEffect::StartListening => START_WAV.get_or_init(|| {
            // Short rising tick. Quiet enough to sit under speech.
            chirp_wav(520.0, 880.0, 0.055, 0.11)
        }),
        SoundEffect::StopListening => STOP_WAV.get_or_init(|| chirp_wav(760.0, 420.0, 0.048, 0.09)),
        SoundEffect::Success => SUCCESS_WAV.get_or_init(|| {
            let sr = 22_050;
            let a = tone(659.25, 0.042, 0.10, sr);
            let b = tone(987.77, 0.070, 0.09, sr);
            voice_stt_desktop::wav::encode_mono(
                &crossfade(&a, &b, (0.008 * sr as f32) as usize),
                sr,
            )
        }),
        SoundEffect::Error => ERROR_WAV.get_or_init(|| chirp_wav(220.0, 146.0, 0.090, 0.10)),
    }
}

fn chirp_wav(f0: f32, f1: f32, dur: f32, amp: f32) -> Vec<u8> {
    let sr = 22_050u32;
    voice_stt_desktop::wav::encode_mono(&chirp(f0, f1, dur, amp, sr), sr)
}

fn chirp(f0: f32, f1: f32, dur: f32, amp: f32, sample_rate: u32) -> Vec<f32> {
    let n = ((dur * sample_rate as f32).round() as usize).max(8);
    let mut out = vec![0.0f32; n];
    let mut phase = 0.0f32;
    let mut noise = 0.17f32;
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / (n as f32 - 1.0);
        let freq = f0 + (f1 - f0) * t;
        phase += 2.0 * PI * freq / sample_rate as f32;
        noise = noise.mul_add(1.37, 0.31).fract();
        let tick = if i < sample_rate as usize / 400 {
            (noise - 0.5) * 0.35
        } else {
            0.0
        };
        let body = phase.sin() + 0.08 * (2.0 * phase).sin();
        let env = soft_env(i, n);
        *sample = (body + tick) * amp * env;
    }
    out
}

fn tone(freq: f32, dur: f32, amp: f32, sample_rate: u32) -> Vec<f32> {
    chirp(freq, freq, dur, amp, sample_rate)
}

fn soft_env(i: usize, n: usize) -> f32 {
    let t = i as f32 / n as f32;
    let attack = (t / 0.08).clamp(0.0, 1.0);
    let release = (-6.5 * t).exp();
    attack * release
}

fn crossfade(a: &[f32], b: &[f32], fade: usize) -> Vec<f32> {
    let fade = fade.min(a.len()).min(b.len()).max(1);
    let mut out = Vec::with_capacity(a.len() + b.len() - fade);
    out.extend_from_slice(&a[..a.len() - fade]);
    for i in 0..fade {
        let t = i as f32 / (fade as f32);
        let av = a[a.len() - fade + i];
        let bv = b[i];
        out.push(av * (1.0 - t) + bv * t);
    }
    out.extend_from_slice(&b[fade..]);
    out
}
