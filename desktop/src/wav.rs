//! 16-bit PCM WAV files: the test takes that TDT writes and `bench-asr`
//! reads, and the sound cues of the app.

/// A mono 16-bit WAV file of `samples` (each from -1 to 1) at `sample_rate`.
pub fn encode_mono(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    // PCM, mono, the rate, its bytes per second, 2-byte frames, 16 bits.
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

/// The audio of a 16-bit PCM WAV file, with its channels mixed to mono, and
/// its sample rate. `None` for any other format.
pub fn decode_mono(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
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
    if bits != 16 || channels == 0 || rate == 0 {
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
    Some((mono, rate))
}
