pub fn format_mmss(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

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
