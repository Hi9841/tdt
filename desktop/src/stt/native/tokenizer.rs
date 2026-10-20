//! Turns token ids back into text with the vocabulary in `tokenizer.json`,
//! and text into token ids for dictionary terms. The model uses Metaspace
//! pieces: `▁` marks the start of a word.

use std::collections::HashMap;
use std::path::Path;

const WORD_START: char = '\u{2581}';

pub struct Tokenizer {
    pieces: Vec<String>,
    special: Vec<bool>,
    ids: HashMap<String, usize>,
    /// Merge rank of each adjacent pair of pieces; lower merges first.
    merges: HashMap<(String, String), usize>,
}

impl Tokenizer {
    pub fn open(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("tokenizer.json is not JSON: {e}"))?;
        let vocab = json["model"]["vocab"]
            .as_object()
            .ok_or("tokenizer.json has no model.vocab")?;
        let size = vocab
            .values()
            .filter_map(|id| id.as_u64())
            .max()
            .map_or(0, |max| max as usize + 1);
        let mut pieces = vec![String::new(); size];
        let mut ids = HashMap::new();
        for (piece, id) in vocab {
            if let Some(id) = id.as_u64() {
                pieces[id as usize] = piece.clone();
                ids.insert(piece.clone(), id as usize);
            }
        }
        let mut special = vec![false; size];
        for added in json["added_tokens"].as_array().into_iter().flatten() {
            let id = added["id"].as_u64().unwrap_or(u64::MAX) as usize;
            if id < size && added["special"].as_bool() == Some(true) {
                special[id] = true;
            }
        }
        let mut merges = HashMap::new();
        for (rank, merge) in json["model"]["merges"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            // Either ["a", "b"] or "a b".
            let pair = match merge {
                serde_json::Value::Array(parts) => parts
                    .first()
                    .and_then(|a| a.as_str())
                    .zip(parts.get(1).and_then(|b| b.as_str())),
                serde_json::Value::String(joined) => joined.split_once(' '),
                _ => None,
            };
            if let Some((a, b)) = pair {
                merges.entry((a.to_string(), b.to_string())).or_insert(rank);
            }
        }
        Ok(Self {
            pieces,
            special,
            ids,
            merges,
        })
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// For each id, whether its piece begins a word. Special ids never do.
    pub fn word_starts(&self) -> Vec<bool> {
        self.pieces
            .iter()
            .zip(&self.special)
            .map(|(piece, special)| !special && piece.starts_with(WORD_START))
            .collect()
    }

    /// Characters in a piece, not counting the word-start mark.
    pub fn letters(&self, id: usize) -> usize {
        self.pieces.get(id).map_or(0, |piece| {
            piece.chars().filter(|c| *c != WORD_START).count()
        })
    }

    /// Unknown and special ids are skipped, as the reference tokenizer does.
    pub fn decode(&self, ids: &[usize]) -> String {
        let mut text = String::new();
        for &id in ids {
            if self.special.get(id).copied().unwrap_or(true) {
                continue;
            }
            text.push_str(&self.pieces[id].replace(WORD_START, " "));
        }
        text.trim_start_matches(' ').to_string()
    }

    /// Token ids of `text` with byte-pair merges, or `None` when a piece is
    /// not in the vocabulary. Each word starts with `▁`.
    pub fn encode(&self, text: &str) -> Option<Vec<usize>> {
        let mut ids = Vec::new();
        for word in text.split_whitespace() {
            let mut symbols: Vec<String> = std::iter::once(WORD_START)
                .chain(word.chars())
                .map(String::from)
                .collect();
            loop {
                let best = symbols
                    .windows(2)
                    .enumerate()
                    .filter_map(|(i, pair)| {
                        self.merges
                            .get(&(pair[0].clone(), pair[1].clone()))
                            .map(|rank| (*rank, i))
                    })
                    .min();
                let Some((_, at)) = best else {
                    break;
                };
                let right = symbols.remove(at + 1);
                symbols[at].push_str(&right);
            }
            for symbol in &symbols {
                ids.push(*self.ids.get(symbol)?);
            }
        }
        Some(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::Tokenizer;

    #[test]
    fn pieces_join_into_words_and_specials_are_skipped() {
        let json = r#"{"model":{"vocab":{"<unk>":0,"▁Send":1,"▁a":2,"▁rep":3,"ort":4,".":5}},
            "added_tokens":[{"id":0,"content":"<unk>","special":true}]}"#;
        let tokenizer = Tokenizer::parse(json).expect("parse");
        assert_eq!(tokenizer.decode(&[1, 2, 0, 3, 4, 5]), "Send a report.");
        assert_eq!(tokenizer.decode(&[99]), "");
    }

    #[test]
    fn merges_apply_by_rank_within_each_word() {
        // "low" merges l+o first (rank 0), then lo+w (rank 2); "ow" (rank 1) loses
        // because "o" is already taken.
        let json = r#"{"model":{"vocab":{"▁":0,"l":1,"o":2,"w":3,"▁lo":4,"lo":5,"ow":6,"▁low":7,"▁l":8},
            "merges":[["l","o"],["o","w"],["▁","lo"],["▁lo","w"]]},"added_tokens":[]}"#;
        let tokenizer = Tokenizer::parse(json).expect("parse");
        assert_eq!(tokenizer.encode("low"), Some(vec![7]));
        assert_eq!(tokenizer.encode("lo low"), Some(vec![4, 7]));
        assert_eq!(tokenizer.encode("x"), None);
    }
}
