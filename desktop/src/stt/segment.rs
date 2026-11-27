//! A take longer than one window runs through the model in overlapping
//! windows while the user still talks, so only the last window waits for the
//! release. Quality comes first, on every machine: the windows do not depend
//! on the speed of the CPU, so the same speech gives the same text.
//!
//! A window is a fast path that must prove itself. The model can drop speech
//! near the edge of a window, so two windows share `OVERLAP_SAMPLES` of speech,
//! and `merge` joins their text in the middle of the shared words, far from
//! both edges. When they do not clearly hold the same words, there is no join:
//! the whole take runs once after the release. That take waits longer and
//! loses nothing.
//!
//! On 400 long LibriSpeech takes, against one run of the whole take: 16 s
//! windows with 4 s shared added 106 errors to 1,228, 24 s with 8 s added 46,
//! and 30 s with 8 s added 15, inside the noise.

use crate::SAMPLE_RATE;

/// The model reads this much speech per run. A take up to this long is one
/// run, the same as without windows.
pub const WINDOW_SAMPLES: usize = 30 * SAMPLE_RATE;
/// Speech that two windows share.
pub const OVERLAP_SAMPLES: usize = 8 * SAMPLE_RATE;
/// Words of each text that can hold the shared speech: 8 words a second.
const SHARED_WORDS: usize = 64;
const MATCH_SCORE: i32 = 2;
const SKIP_COST: i32 = 1;
/// Normal speech puts about 20 words in the shared speech. With fewer equal
/// words than this, a window lost speech, or the overlap is a pause.
const MIN_MATCHED_WORDS: usize = 8;

/// 10 ms.
const FRAME_SAMPLES: usize = SAMPLE_RATE / 100;
/// Users stop talking before they release the key: the median take ends in
/// 375 ms of quiet. After this much quiet the model reads the take, so the
/// text is ready when the key comes up.
const READ_AFTER_QUIET_FRAMES: usize = 20;
/// Models that append their own digital quiet (`ModelSpec::end_quiet_samples`)
/// do not need 200 ms of captured silence to finish the last word, so the
/// pre-release read can start after 100 ms of quiet.
const READ_AFTER_QUIET_PADDED_FRAMES: usize = 10;
/// A frame this far over the room noise is speech. A soft word ending counts
/// as speech: a word after the read must not be lost.
const SPEECH_OVER_NOISE_DB: f32 = 8.0;
/// Once quiet has started, require 30 ms of consecutive active frames before
/// breaking quiet so a 10-20 ms key-release click or noise spike (even one
/// that straddles two 10 ms frames) does not discard a quiet read.
const RESUME_SPEECH_FRAMES: usize = 3;
/// The room noise needs some audio to measure.
const MIN_FRAMES: usize = 60;
const OPENING_FRAMES: usize = 8;

/// Finds where the speech of a take ended, while the take grows.
pub struct EndOfSpeech {
    levels: Vec<f32>,
    /// `levels` in rising order. A sort of the whole take for each chunk of
    /// audio would grow with the take.
    sorted: Vec<f32>,
    /// The level in the middle of the first `OPENING_FRAMES` frames.
    opening: f32,
    /// The frame after the last speech.
    speech_end: usize,
    /// Frames of quiet required before starting a quiet read.
    quiet_frames: usize,
    /// Consecutive active frames seen while already in quiet.
    resume_run: usize,
}

impl Default for EndOfSpeech {
    fn default() -> Self {
        Self::new()
    }
}

impl EndOfSpeech {
    pub fn new() -> Self {
        Self::with_quiet_frames(READ_AFTER_QUIET_FRAMES)
    }

    /// For models that append digital quiet after the captured audio.
    pub fn for_padded_end() -> Self {
        Self::with_quiet_frames(READ_AFTER_QUIET_PADDED_FRAMES)
    }

    fn with_quiet_frames(quiet_frames: usize) -> Self {
        Self {
            levels: Vec::new(),
            sorted: Vec::new(),
            opening: 0.0,
            speech_end: 0,
            quiet_frames,
            resume_run: 0,
        }
    }

    /// `samples` is the whole take so far. Each frame is judged alone, with
    /// the room noise of the take up to that frame. So the speech end does
    /// not depend on where the capture cut the take into chunks.
    pub fn update(&mut self, samples: &[f32]) {
        let first = self.levels.len();
        let (frames, _) = samples[first * FRAME_SAMPLES..].as_chunks::<FRAME_SAMPLES>();
        for frame in frames {
            let level = frame_db(frame);
            let place = self
                .sorted
                .partition_point(|other| other.total_cmp(&level).is_le());
            self.sorted.insert(place, level);
            self.levels.push(level);
            if self.levels.len() == OPENING_FRAMES {
                let mut opening = self.levels.clone();
                opening.sort_by(f32::total_cmp);
                self.opening = opening[OPENING_FRAMES / 2];
            }
            if self.levels.len() < MIN_FRAMES {
                continue;
            }
            let threshold = self.room_noise() + SPEECH_OVER_NOISE_DB;
            if self.levels.len() == MIN_FRAMES {
                if let Some(last) = self.levels.iter().rposition(|&db| db >= threshold) {
                    self.speech_end = last + 1;
                }
                continue;
            }
            let active = level >= threshold;
            let in_quiet = self.speech_end > 0
                && (self.levels.len() - 1).saturating_sub(self.speech_end) >= self.quiet_frames;
            if in_quiet {
                if active {
                    self.resume_run += 1;
                    if self.resume_run >= RESUME_SPEECH_FRAMES {
                        self.speech_end = self.levels.len();
                        self.resume_run = 0;
                    }
                } else {
                    self.resume_run = 0;
                }
            } else if active {
                self.speech_end = self.levels.len();
                self.resume_run = 0;
            }
        }
    }

    /// The lower of the 15th percentile of the take and the first 80 ms,
    /// before the user talks. A short take with no pause has no quiet frames,
    /// and its quietest frames are soft speech.
    fn room_noise(&self) -> f32 {
        let quietest = self.sorted[(self.sorted.len() * 3) / 20];
        quietest.min(self.opening)
    }

    /// The frame where the last speech ended, once enough quiet follows it.
    pub fn quiet_after(&self) -> Option<usize> {
        let quiet = self.levels.len() - self.speech_end;
        (self.speech_end > 0 && quiet >= self.quiet_frames).then_some(self.speech_end)
    }

    /// The samples of the take that the read in the quiet gets for the
    /// speech end `end`: the take up to the end of the quiet that
    /// `quiet_after` waits for. The length comes from the audio only. A read
    /// of all the audio so far has a length that follows the chunks of the
    /// capture, and 10 ms more audio can change the text.
    pub fn read_samples(&self, end: usize) -> usize {
        (end + self.quiet_frames) * FRAME_SAMPLES
    }
}

fn frame_db(frame: &[f32]) -> f32 {
    let power = frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32;
    10.0 * (power + 1e-12).log10()
}

/// `next` is the text of the window after `text`. Keeps `text` up to the
/// middle of the words that both hold, and `next` from there on. `None` when
/// the two do not clearly hold the same speech. A window with no text next to
/// one with text is such a case: the model can write nothing for clear speech.
pub fn merge(text: &str, next: &str) -> Option<String> {
    if text.is_empty() && next.is_empty() {
        return Some(String::new());
    }
    let first: Vec<&str> = text.split_whitespace().collect();
    let second: Vec<&str> = next.split_whitespace().collect();
    let tail = first.len().saturating_sub(SHARED_WORDS);
    let head = second.len().min(SHARED_WORDS);
    let matched = shared_words(&first[tail..], &second[..head]);
    if matched.len() < MIN_MATCHED_WORDS {
        return None;
    }
    let (kept, from) = matched[matched.len() / 2];
    let words = first[..tail + kept].iter().chain(&second[from..]);
    Some(words.copied().collect::<Vec<_>>().join(" "))
}

/// The best line-up of the end of `first` with the start of `second`, as
/// the index pairs of equal words. The text before the line-up and after it
/// costs nothing; inside it, each skipped or different word costs. So a word
/// that repeats far away cannot pull the two texts together and lose the
/// speech between.
fn shared_words(first: &[&str], second: &[&str]) -> Vec<(usize, usize)> {
    #[derive(Clone, Copy)]
    enum Step {
        Both,
        First,
        Second,
    }
    let first: Vec<String> = first.iter().map(|word| plain(word)).collect();
    let second: Vec<String> = second.iter().map(|word| plain(word)).collect();
    let width = second.len() + 1;
    let mut score = vec![0i32; (first.len() + 1) * width];
    let mut step = vec![Step::Both; (first.len() + 1) * width];
    for j in 1..width {
        score[j] = score[j - 1] - SKIP_COST;
        step[j] = Step::Second;
    }
    for i in 1..=first.len() {
        // `score[i * width]` stays 0: the line-up can start at any word of `first`.
        step[i * width] = Step::First;
        for j in 1..width {
            let same = first[i - 1] == second[j - 1];
            let both = score[(i - 1) * width + j - 1] + if same { MATCH_SCORE } else { -SKIP_COST };
            let skip_first = score[(i - 1) * width + j] - SKIP_COST;
            let skip_second = score[i * width + j - 1] - SKIP_COST;
            let cell = i * width + j;
            (score[cell], step[cell]) = if both >= skip_first && both >= skip_second {
                (both, Step::Both)
            } else if skip_first >= skip_second {
                (skip_first, Step::First)
            } else {
                (skip_second, Step::Second)
            };
        }
    }
    // The line-up ends with the last word of `first`, at any word of `second`.
    let last = first.len() * width;
    let mut j = (0..width).max_by_key(|j| score[last + j]).unwrap_or(0);
    let mut i = first.len();
    let mut pairs = Vec::new();
    while i > 0 && j > 0 {
        match step[i * width + j] {
            Step::Both => {
                i -= 1;
                j -= 1;
                if first[i] == second[j] {
                    pairs.push((i, j));
                }
            }
            Step::First => i -= 1,
            Step::Second => j -= 1,
        }
    }
    pairs.reverse();
    pairs
}

/// A word without its case and punctuation.
fn plain(word: &str) -> String {
    word.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames_at(level: f32, count: usize) -> Vec<f32> {
        vec![level; count * FRAME_SAMPLES]
    }

    #[test]
    fn short_take_speech_before_min_frames_still_triggers_quiet() {
        let mut samples = frames_at(0.002, 10);
        samples.extend(frames_at(0.05, 30)); // 100..400 ms speech
        samples.extend(frames_at(0.002, 30)); // 400..700 ms quiet

        let mut speech = EndOfSpeech::for_padded_end();
        speech.update(&samples);
        assert_eq!(speech.quiet_after(), Some(40));
    }

    #[test]
    fn key_click_straddling_two_frames_in_quiet_does_not_discard_quiet_read() {
        let mut samples = frames_at(0.002, 10);
        samples.extend(frames_at(0.05, 70)); // 100..800 ms speech
        samples.extend(frames_at(0.002, 15)); // 800..950 ms quiet
                                              // 10 ms click offset by half a frame so it spans two 10 ms frames.
        samples.extend(vec![0.002; FRAME_SAMPLES / 2]);
        samples.extend(vec![0.05; FRAME_SAMPLES]);
        samples.extend(vec![0.002; FRAME_SAMPLES / 2 + FRAME_SAMPLES]);

        let mut speech = EndOfSpeech::for_padded_end();
        speech.update(&samples);
        assert_eq!(
            speech.quiet_after(),
            Some(80),
            "a 10 ms click straddling two frames after quiet started must keep the quiet point"
        );
    }

    #[test]
    fn real_resumed_speech_breaks_quiet() {
        let mut samples = frames_at(0.002, 10);
        samples.extend(frames_at(0.05, 70));
        samples.extend(frames_at(0.002, 15));
        samples.extend(frames_at(0.05, RESUME_SPEECH_FRAMES));

        let mut speech = EndOfSpeech::for_padded_end();
        speech.update(&samples);
        assert_eq!(speech.quiet_after(), None);
    }

    #[test]
    fn end_of_speech_is_independent_of_chunk_size() {
        let mut samples = frames_at(0.002, 10);
        samples.extend(frames_at(0.05, 55));
        samples.extend(frames_at(0.002, 12));
        samples.extend(frames_at(0.05, 1));
        samples.extend(frames_at(0.002, 8));

        let mut whole = EndOfSpeech::for_padded_end();
        whole.update(&samples);

        let mut chunked = EndOfSpeech::for_padded_end();
        let mut acc = Vec::new();
        for chunk in samples.chunks(FRAME_SAMPLES) {
            acc.extend_from_slice(chunk);
            chunked.update(&acc);
        }
        assert_eq!(whole.quiet_after(), chunked.quiet_after());
    }
}
