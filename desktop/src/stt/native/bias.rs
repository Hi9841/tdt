//! Dictionary terms, boosted during greedy decoding.
//!
//! Each term is a path of token ids in a trie. At a decoding step, a token
//! that continues the term matched so far, or starts a new term, gets a
//! bonus on its logit, but only when the model already scores it within
//! `margin` of its best choice. Speech that does not contain the term keeps
//! its own words, because a gap larger than the margin is never closed.
//!
//! Short pieces such as "▁P" or "▁Car" begin thousands of ordinary words.
//! A term therefore starts only on a first piece with `min_start` letters,
//! only where the model itself starts a word, and a continuation is boosted
//! only once `min_prefix` letters of the term are matched.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiasWeights {
    /// Bonus for the first token of a term.
    pub start: f32,
    /// Bonus for a token that continues a matched prefix.
    pub cont: f32,
    /// A token more than this far below the best logit gets no bonus.
    pub margin: f32,
    /// Letters a first piece needs before it may get the start bonus.
    pub min_start: usize,
    /// Letters of a term that must be matched before continuations get a bonus.
    pub min_prefix: usize,
}

impl Default for BiasWeights {
    fn default() -> Self {
        Self {
            start: 4.0,
            cont: 8.0,
            margin: 10.0,
            min_start: 4,
            min_prefix: 3,
        }
    }
}

#[derive(Default)]
struct Node {
    children: HashMap<usize, usize>,
    /// Letters from the root to this node.
    letters: usize,
    /// Tokens from the root to this node.
    depth: usize,
    /// A term ends here; the index into `replacements`.
    ends: Option<usize>,
}

/// One path to boost, and the tokens to write instead when it completes.
pub struct Term {
    pub path: Vec<usize>,
    /// The term as the user wrote it, when this path spells it differently.
    pub written: Option<Vec<usize>>,
    /// Write `written` even when the model chose this path without help. True
    /// for splits and inner capitals ("web socket" -> "WebSocket"); false for a
    /// lowercase first letter, which may be an ordinary word ("vault").
    pub always: bool,
}

/// A term finished at the last emitted token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    /// Emitted tokens that belong to the term.
    pub len: usize,
    replacement: usize,
}

pub struct Bias {
    nodes: Vec<Node>,
    replacements: Vec<(Option<Vec<usize>>, bool)>,
    startable: HashSet<usize>,
    /// Tokens that begin a word.
    word_start: Vec<bool>,
    pub weights: BiasWeights,
}

/// Position in the trie between decoding steps; 0 is the root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BiasState(usize);

impl Bias {
    /// `terms` are token-id paths (empty ones are ignored); `letters[id]` and
    /// `word_start[id]` describe each token's piece.
    pub fn new(
        terms: impl IntoIterator<Item = Term>,
        letters: &[usize],
        word_start: Vec<bool>,
        weights: BiasWeights,
    ) -> Self {
        let letters_of = |token: usize| letters.get(token).copied().unwrap_or(0);
        let mut nodes = vec![Node::default()];
        let mut replacements = Vec::new();
        let mut startable = HashSet::new();
        for Term {
            path: term,
            written,
            always,
        } in terms
        {
            if term.is_empty() {
                continue;
            }
            if let Some(&first) = term.first() {
                if letters_of(first) >= weights.min_start {
                    startable.insert(first);
                }
            }
            let mut at = 0;
            for token in term {
                let next = nodes.len();
                let child = *nodes[at].children.entry(token).or_insert(next);
                if child == next {
                    let prefix = nodes[at].letters + letters_of(token);
                    let depth = nodes[at].depth + 1;
                    nodes.push(Node {
                        letters: prefix,
                        depth,
                        ..Node::default()
                    });
                }
                at = child;
            }
            if nodes[at].ends.is_none() {
                nodes[at].ends = Some(replacements.len());
                replacements.push((written, always));
            }
        }
        Self {
            nodes,
            replacements,
            startable,
            word_start,
            weights,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes[0].children.is_empty()
    }

    /// Adds the bonuses to `logits` (token part only) for the next step.
    pub fn apply(&self, state: BiasState, logits: &mut [f32]) {
        let (best_token, best) =
            logits
                .iter()
                .copied()
                .enumerate()
                .fold(
                    (0, f32::NEG_INFINITY),
                    |a, (i, v)| if v > a.1 { (i, v) } else { a },
                );
        let floor = best - self.weights.margin;
        let mut boost = |token: usize, bonus: f32| {
            if let Some(logit) = logits.get_mut(token) {
                if *logit >= floor {
                    *logit += bonus;
                }
            }
        };
        let node = &self.nodes[state.0];
        if state.0 != 0 && node.letters >= self.weights.min_prefix {
            for &token in node.children.keys() {
                boost(token, self.weights.cont);
            }
        }
        // A term starts only where the model itself starts a word, so a boost
        // never splits the word it is spelling.
        if !self.word_start.get(best_token).copied().unwrap_or(false) {
            return;
        }
        for &token in &self.startable {
            // Children of a matched prefix already got the continuation bonus.
            if state.0 == 0 || !node.children.contains_key(&token) {
                boost(token, self.weights.start);
            }
        }
    }

    /// The state after `token` was emitted, and the term it completes, if any.
    pub fn advance(&self, state: BiasState, token: usize) -> (BiasState, Option<Completion>) {
        let next = self.nodes[state.0]
            .children
            .get(&token)
            .or_else(|| self.nodes[0].children.get(&token));
        let Some(&next) = next else {
            return (BiasState(0), None);
        };
        let node = &self.nodes[next];
        let completion = node.ends.map(|replacement| Completion {
            len: node.depth,
            replacement,
        });
        let state = if node.children.is_empty() { 0 } else { next };
        (BiasState(state), completion)
    }

    /// The tokens to write for a completed term, when they differ from the
    /// path. `decisive` is whether a bonus changed a choice inside the term.
    pub fn replacement(&self, completion: Completion, decisive: bool) -> Option<&[usize]> {
        let (written, always) = &self.replacements[completion.replacement];
        written.as_deref().filter(|_| *always || decisive)
    }

    pub fn starts_word(&self, token: usize) -> bool {
        self.word_start.get(token).copied().unwrap_or(false)
    }
}

/// Spellings the model may produce for a term, each with whether to write
/// the term as given when that spelling is decoded.
///
/// - The term itself.
/// - All lowercase ("postgres" for "Postgres"). Rewritten only when the term
///   has capitals after its first letter ("PostgreSQL", "GitHub", "AWS"): a
///   difference in the first letter alone may be an ordinary word ("vault").
/// - CamelCase split into words ("web socket" for "WebSocket", "graph QL"
///   for "GraphQL"), lowercase except all-capital parts. Always rewritten.
pub fn variants(term: &str) -> Vec<(String, bool)> {
    let inner_capitals = term
        .split_whitespace()
        .any(|word| word.chars().skip(1).any(char::is_uppercase));
    let mut out = vec![(term.to_string(), false)];
    let lower = term.to_lowercase();
    if lower != term {
        out.push((lower, inner_capitals));
    }
    let split: Vec<String> = term
        .split_whitespace()
        .flat_map(|word| {
            let chars: Vec<char> = word.chars().collect();
            let mut parts = vec![String::new()];
            for (i, c) in chars.iter().enumerate() {
                let hump = i > 0 && c.is_uppercase() && chars[i - 1].is_lowercase();
                if hump {
                    parts.push(String::new());
                }
                if let Some(part) = parts.last_mut() {
                    part.push(*c);
                }
            }
            parts
        })
        .map(|part| {
            if part.chars().all(|c| !c.is_lowercase()) {
                part
            } else {
                part.to_lowercase()
            }
        })
        .collect();
    let split = split.join(" ");
    if split.contains(' ') && split.to_lowercase() != term.to_lowercase() {
        out.push((split, true));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Bias, BiasState, BiasWeights, Term};

    fn weights() -> BiasWeights {
        BiasWeights {
            start: 1.0,
            cont: 3.0,
            margin: 2.0,
            min_start: 4,
            min_prefix: 3,
        }
    }

    fn plain(path: &[usize]) -> Term {
        Term {
            path: path.to_vec(),
            written: None,
            always: false,
        }
    }

    /// Ten tokens; token i has `letters[i]` letters and every one starts a word.
    fn bias(terms: &[&[usize]], letters: [usize; 10]) -> Bias {
        Bias::new(
            terms.iter().map(|t| plain(t)),
            &letters,
            vec![true; 10],
            weights(),
        )
    }

    fn state(bias: &Bias, from: BiasState, token: usize) -> BiasState {
        bias.advance(from, token).0
    }

    #[test]
    fn a_continuation_inside_the_margin_wins_and_a_far_one_does_not() {
        let bias = bias(&[&[5, 6, 7]], [4; 10]);
        let state = state(&bias, BiasState::default(), 5);
        let mut close = vec![0.0f32; 10];
        close[1] = 1.0;
        close[6] = -0.5;
        bias.apply(state, &mut close);
        assert_eq!(close[6], 2.5);
        let mut far = vec![0.0f32; 10];
        far[1] = 5.0;
        bias.apply(state, &mut far);
        assert_eq!(far[6], 0.0);
    }

    #[test]
    fn states_follow_terms_and_reset_after_the_end_or_a_break() {
        let bias = bias(&[&[5, 6, 7], &[5, 8]], [4; 10]);
        let s = state(&bias, BiasState::default(), 5);
        assert_ne!(s, BiasState::default());
        let s2 = state(&bias, s, 6);
        assert_eq!(state(&bias, s2, 7), BiasState::default());
        assert_eq!(state(&bias, s, 8), BiasState::default());
        assert_eq!(state(&bias, s2, 5), s);
        assert_eq!(state(&bias, s2, 9), BiasState::default());
        assert!(Bias::new(Vec::<Term>::new(), &[], vec![], weights()).is_empty());
    }

    #[test]
    fn short_pieces_neither_start_a_term_nor_unlock_its_continuation() {
        // Term 3 -> 4 -> 5 with a 1-letter first piece; term 9 -> 4 with a long one.
        let mut letters = [4; 10];
        letters[3] = 1;
        letters[4] = 2;
        let bias = bias(&[&[3, 4, 5], &[9, 4]], letters);
        let mut logits = vec![0.0f32; 10];
        bias.apply(BiasState::default(), &mut logits);
        assert_eq!((logits[3], logits[9]), (0.0, 1.0));
        // After "3" only 1 letter is matched: no continuation bonus yet.
        let after_one = state(&bias, BiasState::default(), 3);
        let mut logits = vec![0.0f32; 10];
        bias.apply(after_one, &mut logits);
        assert_eq!(logits[4], 0.0);
        // After "3 4" three letters are matched: "5" gets the bonus.
        let after_two = state(&bias, after_one, 4);
        let mut logits = vec![0.0f32; 10];
        bias.apply(after_two, &mut logits);
        assert_eq!(logits[5], 3.0);
    }

    #[test]
    fn no_term_starts_while_the_model_is_inside_a_word() {
        let mut word_start = vec![true; 10];
        word_start[1] = false;
        let bias = Bias::new([plain(&[9, 4])], &[4; 10], word_start, weights());
        let mut logits = vec![0.0f32; 10];
        logits[1] = 1.0;
        bias.apply(BiasState::default(), &mut logits);
        assert_eq!(logits[9], 0.0);
        let mut logits = vec![0.0f32; 10];
        logits[2] = 1.0;
        bias.apply(BiasState::default(), &mut logits);
        assert_eq!(logits[9], 1.0);
    }

    #[test]
    fn a_completed_variant_reports_its_length_and_replacement() {
        // Variant 2 -> 3 stands for the written form 7.
        let terms = [
            plain(&[5, 6]),
            Term {
                path: vec![2, 3],
                written: Some(vec![7]),
                always: false,
            },
        ];
        let bias = Bias::new(terms, &[4; 10], vec![true; 10], weights());
        let (s, none) = bias.advance(BiasState::default(), 2);
        assert_eq!(none, None);
        let (_, done) = bias.advance(s, 3);
        let done = done.expect("completion");
        assert_eq!(done.len, 2);
        // A lowercase variant is rewritten only when the bonus decided it.
        assert_eq!(bias.replacement(done, true), Some(&[7][..]));
        assert_eq!(bias.replacement(done, false), None);
        let (s, _) = bias.advance(BiasState::default(), 5);
        let (_, plain_done) = bias.advance(s, 6);
        assert_eq!(
            bias.replacement(plain_done.expect("completion"), true),
            None
        );
    }

    #[test]
    fn variants_cover_lowercase_and_camel_case_splits() {
        use super::variants;
        assert_eq!(
            variants("WebSocket"),
            vec![
                ("WebSocket".to_string(), false),
                ("websocket".to_string(), true),
                ("web socket".to_string(), true),
            ]
        );
        assert_eq!(variants("GraphQL")[2], ("graph QL".to_string(), true));
        // A first capital alone is not rewritten, and no split exists.
        assert_eq!(
            variants("Vault"),
            vec![("Vault".to_string(), false), ("vault".to_string(), false)]
        );
        assert_eq!(variants("npm"), vec![("npm".to_string(), false)]);
    }
}
