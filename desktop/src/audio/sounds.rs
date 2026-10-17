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
    /// Every cue. The prewarm test matches each one, so a new cue fails the build
    /// until it is listed here and synthesized in `wav_for`.
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

    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

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
            pcm_to_wav(&crossfade(&a, &b, (0.008 * sr as f32) as usize), sr)
        }),
        SoundEffect::Error => ERROR_WAV.get_or_init(|| chirp_wav(220.0, 146.0, 0.090, 0.10)),
    }
}

fn chirp_wav(f0: f32, f1: f32, dur: f32, amp: f32) -> Vec<u8> {
    let sr = 22_050u32;
    pcm_to_wav(&chirp(f0, f1, dur, amp, sr), sr)
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

fn pcm_to_wav(pcm: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::with_capacity(pcm.len());
    for &s in pcm {
        samples.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let file_len = 36 + data_len;
    let mut wav = Vec::with_capacity(44 + samples.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_len.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_has_riff_header_and_body() {
        let wav = chirp_wav(440.0, 550.0, 0.04, 0.1);
        assert!(wav.starts_with(b"RIFF"));
        assert!(wav.len() > 44);
        assert!(wav.len() < 8_000);
        let peak = wav[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 200 && peak < 12_000);
    }

    #[test]
    fn prewarm_covers_every_effect() {
        let mut mask = 0u8;
        for effect in SoundEffect::ALL {
            mask |= match effect {
                SoundEffect::StartListening => 1,
                SoundEffect::StopListening => 2,
                SoundEffect::Success => 4,
                SoundEffect::Error => 8,
            };
            assert!(wav_for(effect).starts_with(b"RIFF"));
        }
        assert_eq!(mask, 0b1111);
        assert_eq!(SoundEffect::ALL.len(), 4);
    }
}
