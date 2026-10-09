//! Removes hesitation sounds ("um", "uh", "erm", "hmm") from a transcript.
//!
//! Only sounds that never carry meaning are removed. Words such as "like",
//! "you know", "so", and "ah" can be real speech, so they stay. The commas the
//! recognizer puts around a filler go with it, and a sentence that started on
//! a filler still starts with a capital letter.

pub fn remove_fillers(text: &str) -> String {
    if !text
        .split_whitespace()
        .any(|token| is_filler(core(token).1))
    {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    // Opening quotes or brackets of a removed filler move to the next word.
    let mut carried_lead = String::new();
    let mut capitalize_next = false;

    for (gap, token) in tokens(text) {
        let (lead, word, trail) = core(token);
        if !is_filler(word) {
            if !out.is_empty() {
                out.push_str(gap);
            }
            out.push_str(&carried_lead);
            carried_lead.clear();
            if capitalize_next {
                push_capitalized(&mut out, token);
                capitalize_next = false;
            } else {
                out.push_str(token);
            }
            continue;
        }

        let starts_sentence = ends_sentence(out.trim_end());
        if starts_sentence && word.chars().next().is_some_and(char::is_uppercase) {
            capitalize_next = true;
        }
        carried_lead.push_str(lead);

        if let Some(mark) = terminal_mark(trail) {
            // "That's it, um." keeps its full stop: "That's it."
            if out.ends_with([',', ';']) {
                out.pop();
            }
            if !out.is_empty() && !ends_sentence(&out) {
                out.push(mark);
            }
        } else if trail.contains(',') && out.ends_with(',') {
            // "We should, uh, go" was one phrase: "We should go".
            out.pop();
        }
    }
    // A quote opened on a final filler has no word to land on, so it is
    // dropped rather than left unbalanced.
    out
}

/// Whitespace before each token, then the token.
fn tokens(text: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let start = rest.find(|c: char| !c.is_whitespace())?;
        let gap = &rest[..start];
        let after_gap = &rest[start..];
        let end = after_gap
            .find(char::is_whitespace)
            .unwrap_or(after_gap.len());
        let token = &after_gap[..end];
        rest = &after_gap[end..];
        Some((gap, token))
    })
}

/// Split a token into leading punctuation, the word, and trailing punctuation.
fn core(token: &str) -> (&str, &str, &str) {
    let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '-';
    let start = token.find(is_word).unwrap_or(token.len());
    let end = token.rfind(is_word).map_or(start, |index| {
        index + token[index..].chars().next().map_or(1, char::len_utf8)
    });
    (&token[..start], &token[start..end], &token[end..])
}

fn is_filler(word: &str) -> bool {
    if word.is_empty() || word.len() > 12 {
        return false;
    }
    let lower = word.to_lowercase();
    let mut collapsed = String::with_capacity(lower.len());
    for c in lower.chars() {
        if !collapsed.ends_with(c) {
            collapsed.push(c);
        }
    }
    match collapsed.as_str() {
        // um, umm, uh, uhh, uhm, uhmm, hm, hmm, erm, errm
        "um" | "uh" | "uhm" | "hm" | "erm" => true,
        // "err" is also a verb; only the bare sound is a filler.
        "er" => lower == "er",
        // "mm" and "mmm", but not the letter "m".
        "m" => lower.len() >= 2,
        _ => false,
    }
}

fn terminal_mark(trail: &str) -> Option<char> {
    if trail.contains("..") || trail.contains('…') {
        return None;
    }
    trail.chars().rev().find(|c| matches!(c, '.' | '?' | '!'))
}

fn ends_sentence(text: &str) -> bool {
    text.is_empty()
        || text
            .trim_end_matches(['"', '\'', ')', ']', '\u{201d}', '\u{2019}'])
            .ends_with(['.', '?', '!', '\n'])
}

fn push_capitalized(out: &mut String, token: &str) {
    let lead_len = token.len()
        - token
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .len();
    out.push_str(&token[..lead_len]);
    let mut chars = token[lead_len..].chars();
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(chars.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::remove_fillers;

    #[test]
    fn removes_hesitations_and_the_commas_around_them() {
        assert_eq!(
            remove_fillers(
                "So, um, I think we should, uh, go to the store, and, um, buy some milk. Uh, you know, it's, like, really important."
            ),
            "So I think we should go to the store, and buy some milk. You know, it's, like, really important."
        );
    }

    #[test]
    fn text_without_fillers_is_unchanged() {
        let text = "Ship it  today.\nThen rest, I guess.";
        assert_eq!(remove_fillers(text), text);
    }

    #[test]
    fn a_sentence_that_started_on_a_filler_keeps_its_capital() {
        assert_eq!(remove_fillers("Um, yes."), "Yes.");
        assert_eq!(remove_fillers("Fine. Uh, next one?"), "Fine. Next one?");
        assert_eq!(remove_fillers("Uh, uh, I want that."), "I want that.");
    }

    #[test]
    fn a_filler_at_the_end_hands_over_its_full_stop() {
        assert_eq!(remove_fillers("That's it, um."), "That's it.");
        assert_eq!(remove_fillers("Is that all, uh?"), "Is that all?");
        assert_eq!(remove_fillers("Done. Um."), "Done.");
    }

    #[test]
    fn repeated_and_stretched_fillers_go_too() {
        assert_eq!(remove_fillers("Um um umm ummm hello"), "Hello");
        assert_eq!(
            remove_fillers("I was, uhhh, hmm, thinking"),
            "I was thinking"
        );
        assert_eq!(remove_fillers("Erm, mmm, okay."), "Okay.");
    }

    #[test]
    fn only_fillers_leaves_nothing() {
        assert_eq!(remove_fillers("Um."), "");
        assert_eq!(remove_fillers("uh, um..."), "");
    }

    #[test]
    fn words_that_carry_meaning_stay() {
        for text in [
            "I like it, you know.",
            "Uh-huh, that works.",
            "Mhm, sure.",
            "To err is human.",
            "Plan B or plan M.",
            "Ah, I see.",
            "The UMass campus.",
            "Hummus please.",
        ] {
            assert_eq!(remove_fillers(text), text);
        }
    }

    #[test]
    fn quotes_opened_on_a_filler_move_to_the_next_word() {
        assert_eq!(
            remove_fillers("He said \"um, hello\" to me."),
            "He said \"hello\" to me."
        );
    }

    #[test]
    fn lists_keep_their_colon_and_line_breaks_survive() {
        assert_eq!(
            remove_fillers("Buy: um, eggs\nand, uh, bread"),
            "Buy: eggs\nand bread"
        );
    }
}
