pub fn format_mmss(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
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
    let mut clipped: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_mmss_pads_seconds() {
        assert_eq!(format_mmss(0), "00:00");
        assert_eq!(format_mmss(9), "00:09");
        assert_eq!(format_mmss(75), "01:15");
    }

    #[test]
    fn format_time_saved_uses_compact_units() {
        assert_eq!(format_time_saved(9.0), "9s");
        assert_eq!(format_time_saved(75.0), "1m 15s");
        assert_eq!(format_time_saved(8040.0), "2h 14m");
    }

    #[test]
    fn clip_text_keeps_short_strings_and_ellipsizes_long_ones() {
        assert_eq!(clip_text("hello", 8), "hello");
        assert_eq!(clip_text("  hello world  ", 8), "hello w…");
    }
}
