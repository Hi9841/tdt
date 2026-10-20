//! Turns token ids back into text with the vocabulary in `tokenizer.json`.
//! The model uses Metaspace pieces: `▁` marks the start of a word.

use std::path::Path;

pub struct Tokenizer {
    pieces: Vec<String>,
    special: Vec<bool>,
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
        for (piece, id) in vocab {
            if let Some(id) = id.as_u64() {
                pieces[id as usize] = piece.clone();
            }
        }
        let mut special = vec![false; size];
        for added in json["added_tokens"].as_array().into_iter().flatten() {
            let id = added["id"].as_u64().unwrap_or(u64::MAX) as usize;
            if id < size && added["special"].as_bool() == Some(true) {
                special[id] = true;
            }
        }
        Ok(Self { pieces, special })
    }

    /// Unknown and special ids are skipped, as the reference tokenizer does.
    pub fn decode(&self, ids: &[usize]) -> String {
        let mut text = String::new();
        for &id in ids {
            if self.special.get(id).copied().unwrap_or(true) {
                continue;
            }
            text.push_str(&self.pieces[id].replace('\u{2581}', " "));
        }
        text.trim_start_matches(' ').to_string()
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
}
