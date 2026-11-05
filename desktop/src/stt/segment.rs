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

/// The model reads this much speech per run. A take up to this long is one
/// run, the same as without windows.
pub const WINDOW_SAMPLES: usize = 30 * 16_000;
/// Speech that two windows share.
pub const OVERLAP_SAMPLES: usize = 8 * 16_000;
/// Words of each text that can hold the shared speech: 8 words a second.
const SHARED_WORDS: usize = 64;
const MATCH_SCORE: i32 = 2;
const SKIP_COST: i32 = 1;
/// Normal speech puts about 20 words in the shared speech. With fewer equal
/// words than this, a window lost speech, or the overlap is a pause.
const MIN_MATCHED_WORDS: usize = 8;

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
