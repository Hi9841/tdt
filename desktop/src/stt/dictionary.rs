//! Words and names the engine should spell the user's way: the terms in
//! `%LOCALAPPDATA%\TDT\dictionary.txt`, which the user edits. TDT ships no
//! terms of its own. The file is read again whenever it changes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

const USER_FILE_HEADER: &str = "\
# TDT dictionary
# One word or name per line, written the way you want it (for example: Kubernetes).
# TDT favors these spellings when you dictate them. Lines that start with # are ignored.
# Save the file; the next dictation uses it.
";

/// `%LOCALAPPDATA%\TDT\dictionary.txt`.
pub fn user_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("TDT").join("dictionary.txt"))
}

/// The user file, created with a short explanation when it does not exist.
pub fn ensure_user_file() -> Result<PathBuf, String> {
    let path = user_file().ok_or("LOCALAPPDATA is not set")?;
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, USER_FILE_HEADER)
            .map_err(|e| format!("Could not create {}: {e}", path.display()))?;
    }
    Ok(path)
}

/// Terms from dictionary text: one per line, blank lines and `#` comments skipped.
/// A leading byte order mark, which Windows editors can add, is not part of the first term.
pub fn parse(text: &str) -> Vec<String> {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Remembers the user file's modification time, to reload only after a change.
#[derive(Default)]
pub struct Watch {
    seen: Option<Option<SystemTime>>,
}

impl Watch {
    /// The user's terms when the file changed since the last call (or on the
    /// first call), otherwise `None`. A missing file counts as empty.
    pub fn changed_terms(&mut self, path: Option<&Path>) -> Option<Vec<String>> {
        let modified = path
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if self.seen == Some(modified) {
            return None;
        }
        self.seen = Some(modified);
        let text = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Some(parse(&text))
    }
}
