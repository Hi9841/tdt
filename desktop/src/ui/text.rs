pub fn format_mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn clip_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut clipped: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_mmss_pads_seconds() {
        assert_eq!(format_mmss(0), "0:00");
        assert_eq!(format_mmss(9), "0:09");
        assert_eq!(format_mmss(75), "1:15");
    }

    #[test]
    fn clip_text_keeps_short_strings_and_ellipsizes_long_ones() {
        assert_eq!(clip_text("hello", 8), "hello");
        assert_eq!(clip_text("  hello world  ", 8), "hello w…");
    }
}
