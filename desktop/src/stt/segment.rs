//! Finds pauses where a take can be cut, so finished speech is transcribed
//! while the user still talks and only the last part waits for the release.

/// 10 ms at 16 kHz.
const FRAME: usize = 160;
/// A pause this long is a sentence break in normal speech, not a breath
/// inside a phrase. Shorter cuts change the punctuation the model chooses.
const MIN_PAUSE_FRAMES: usize = 50;
/// The model needs some context. Shorter parts wait for the next pause.
pub const MIN_SEGMENT_SAMPLES: usize = 3 * 16_000;
/// A frame this far under the loud speech of the part is a pause.
const QUIET_BELOW_SPEECH_DB: f32 = 25.0;
/// A part whose loud frames are under this level has no speech to cut.
const MIN_SPEECH_DB: f32 = -45.0;

/// The sample index where `samples[start..]` can be cut, or `None`.
///
/// The cut is in the middle of the first long pause that leaves at least
/// `MIN_SEGMENT_SAMPLES` before it, so both parts keep some silence.
pub fn pause_cut(samples: &[f32], start: usize) -> Option<usize> {
    let part = samples.get(start..)?;
    if part.len() < MIN_SEGMENT_SAMPLES {
        return None;
    }
    let levels: Vec<f32> = part
        .as_chunks::<FRAME>()
        .0
        .iter()
        .map(|frame| frame_db(frame))
        .collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let speech_db = sorted[(sorted.len() * 9) / 10];
    if speech_db < MIN_SPEECH_DB {
        return None;
    }
    let quiet_db = speech_db - QUIET_BELOW_SPEECH_DB;
    let mut run_start = None;
    for (index, level) in levels.iter().enumerate() {
        if *level < quiet_db {
            let first = *run_start.get_or_insert(index);
            if index + 1 - first >= MIN_PAUSE_FRAMES {
                let middle = (first + index).div_ceil(2) * FRAME;
                if middle >= MIN_SEGMENT_SAMPLES {
                    return Some(start + middle);
                }
            }
        } else {
            run_start = None;
        }
    }
    None
}

/// Join the text of the parts. A cut can fall inside a sentence: the model
/// then ends the first part with a period and starts the next part in
/// lowercase. That period becomes a comma.
pub fn join_parts(parts: &[String]) -> String {
    let mut joined = String::new();
    for part in parts {
        let continues = part.chars().next().is_some_and(char::is_lowercase);
        if continues && joined.ends_with('.') && !joined.ends_with("..") {
            joined.pop();
            joined.push(',');
        }
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(part);
    }
    joined
}

fn frame_db(frame: &[f32]) -> f32 {
    let power = frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32;
    10.0 * (power + 1e-12).log10()
}
