pub fn format_latency_ms(latency_ms: u64) -> String {
    format!("{latency_ms}ms")
}

pub fn format_time_saved(secs: f32) -> String {
    let total = secs.max(0.0) as u64;
    if total < 60 {
        format!("{total}s")
    } else if total < 3600 {
        format!("{}m {:02}s", total / 60, total % 60)
    } else {
        format!("{}h {:02}m", total / 3600, (total % 3600) / 60)
    }
}

pub fn clip_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut clipped: String = trimmed.chars().take(max_chars - 1).collect();
    clipped.push('…');
    clipped
}

/// The overlay has room for one short sentence. Details shows the full message.
pub fn first_sentence(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .match_indices(['.', '!', '?'])
        .find(|(end, _)| trimmed[end + 1..].starts_with(char::is_whitespace))
        .map_or(trimmed, |(end, _)| &trimmed[..end])
}

#[cfg(test)]
mod tests {
    use super::first_sentence;

    #[test]
    fn a_multi_sentence_message_keeps_only_its_first_sentence() {
        assert_eq!(
            first_sentence("Could not copy text. Open History to copy the saved transcript."),
            "Could not copy text"
        );
        assert_eq!(
            first_sentence("Microphone disconnected! Connect a microphone."),
            "Microphone disconnected"
        );
    }

    #[test]
    fn a_single_sentence_or_version_stays_whole() {
        assert_eq!(
            first_sentence("Microphone was disconnected"),
            "Microphone was disconnected"
        );
        assert_eq!(first_sentence("v0.2.23 failed."), "v0.2.23 failed.");
        assert_eq!(first_sentence("  Trimmed. "), "Trimmed.");
    }
}
