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

pub fn play_sound(effect: SoundEffect) {
    let wav_bytes: &'static [u8] = match effect {
        SoundEffect::StartListening => START_WAV.get_or_init(|| {
            // Soft rise: A4 -> E5
            two_note_wav(440.0, 659.25, 0.055, 0.075, 0.20)
        }),
        SoundEffect::StopListening => STOP_WAV.get_or_init(|| {
            // Soft fall: E5 -> C5
            two_note_wav(659.25, 523.25, 0.045, 0.070, 0.17)
        }),
        SoundEffect::Success => SUCCESS_WAV.get_or_init(|| {
            // Quiet major third: E5 -> G5
            two_note_wav(659.25, 783.99, 0.050, 0.085, 0.19)
        }),
        SoundEffect::Error => ERROR_WAV.get_or_init(|| {
            // Low muted fall: G3 -> D3
            two_note_wav(196.00, 146.83, 0.070, 0.095, 0.14)
        }),
    };

    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

        unsafe {
            let _ = PlaySoundW(
                PCWSTR(wav_bytes.as_ptr() as *const u16),
                None,
                SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
            );
        }
    }
}

fn two_note_wav(f1: f32, f2: f32, d1: f32, d2: f32, amp: f32) -> Vec<u8> {
    let sr = 44100;
    let a = synth_note(f1, d1, amp, sr);
    let b = synth_note(f2, d2, amp * 1.05, sr);
    pcm_to_wav(&crossfade(&a, &b, (0.006 * sr as f32) as usize), sr)
}

fn synth_note(freq: f32, dur: f32, amp: f32, sample_rate: u32) -> Vec<f32> {
    let n = ((dur * sample_rate as f32).round() as usize).max(8);
    let attack = ((0.006 * sample_rate as f32) as usize).clamp(2, n / 3);
    let release = ((0.40 * n as f32) as usize).clamp(attack, n - 1);
    let mut out = vec![0.0f32; n];
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / sample_rate as f32;
        let phase = 2.0 * PI * freq * t;
        let harmonic = phase.sin() + 0.14 * (2.0 * phase).sin() + 0.04 * (3.0 * phase).sin();
        let a = if i < attack {
            i as f32 / attack as f32
        } else {
            1.0
        };
        let r = if i + release >= n {
            (n - 1 - i) as f32 / release as f32
        } else {
            1.0
        };
        *sample = harmonic * amp * a * r.clamp(0.0, 1.0);
    }
    out
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
        let wav = two_note_wav(440.0, 550.0, 0.04, 0.04, 0.1);
        assert!(wav.starts_with(b"RIFF"));
        assert!(wav.len() > 44);
    }

    #[test]
    fn two_note_is_longer_than_one() {
        let one = pcm_to_wav(&synth_note(440.0, 0.05, 0.1, 44100), 44100);
        let two = two_note_wav(440.0, 550.0, 0.05, 0.05, 0.1);
        assert!(two.len() > one.len());
    }
}
