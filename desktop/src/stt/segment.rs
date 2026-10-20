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

#[cfg(test)]
mod tests {
    use super::{pause_cut, MIN_SEGMENT_SAMPLES};

    fn tone(ms: usize) -> Vec<f32> {
        (0..ms * 16)
            .map(|i| 0.3 * (i as f32 * 0.07).sin())
            .collect()
    }

    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0005; ms * 16]
    }

    #[test]
    fn a_long_pause_after_enough_speech_is_cut_in_its_middle() {
        let mut take = tone(3500);
        take.extend(silence(600));
        take.extend(tone(1000));
        let cut = pause_cut(&take, 0).expect("cut");
        assert!((3500 * 16 + 250 * 16..=3500 * 16 + 350 * 16).contains(&cut));
    }

    #[test]
    fn a_short_pause_or_a_short_part_is_not_cut() {
        let mut breath = tone(3500);
        breath.extend(silence(300));
        breath.extend(tone(1000));
        assert_eq!(pause_cut(&breath, 0), None);

        let mut early = tone(1000);
        early.extend(silence(800));
        early.extend(tone(800));
        assert!(early.len() < MIN_SEGMENT_SAMPLES);
        assert_eq!(pause_cut(&early, 0), None);
    }

    #[test]
    fn the_search_starts_after_the_last_cut() {
        let mut take = tone(3500);
        take.extend(silence(600));
        take.extend(tone(3500));
        take.extend(silence(600));
        take.extend(tone(500));
        let first = pause_cut(&take, 0).expect("first cut");
        let second = pause_cut(&take, first).expect("second cut");
        assert!(second > first + MIN_SEGMENT_SAMPLES);
    }

    #[test]
    fn silence_alone_is_not_cut() {
        assert_eq!(pause_cut(&silence(6000), 0), None);
    }
}

#[cfg(test)]
mod join_tests {
    use super::join_parts;

    #[test]
    fn a_lowercase_continuation_turns_the_cut_period_into_a_comma() {
        let parts = vec![
            "I want to look at the recovery path.".to_string(),
            "because two people reported it.".to_string(),
        ];
        assert_eq!(
            join_parts(&parts),
            "I want to look at the recovery path, because two people reported it."
        );
    }

    #[test]
    fn real_sentence_ends_and_other_marks_stay() {
        let parts = vec![
            "This is done.".to_string(),
            "Next week I start.".to_string(),
            "Is it ready?".to_string(),
            "yes it is".to_string(),
            "Wait...".to_string(),
            "and then".to_string(),
        ];
        assert_eq!(
            join_parts(&parts),
            "This is done. Next week I start. Is it ready? yes it is Wait... and then"
        );
    }
}
