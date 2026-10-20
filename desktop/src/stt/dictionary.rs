//! Words and names the engine should spell the user's way: a built-in list
//! of developer terms plus `%LOCALAPPDATA%\TDT\dictionary.txt`, which the
//! user edits. The file is read again whenever it changes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Common developer names and terms. On by default; measured to change no
/// transcript of ordinary speech (LibriSpeech test subsets).
pub const DEVELOPER_TERMS: &str = include_str!("../../assets/dictionary/developer-terms.txt");

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
pub fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The built-in terms followed by the user's, without repeats.
pub fn merged(user: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    parse(DEVELOPER_TERMS)
        .into_iter()
        .chain(parse(user))
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

/// Remembers the user file's modification time, to reload only after a change.
#[derive(Default)]
pub struct Watch {
    seen: Option<Option<SystemTime>>,
}

impl Watch {
    /// The merged terms when the file changed since the last call (or on the
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
        Some(merged(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::{merged, parse, Watch, DEVELOPER_TERMS};

    #[test]
    fn comments_blank_lines_and_repeats_are_skipped() {
        assert_eq!(
            parse("# note\n\n Kubernetes \nTDT\n#x"),
            vec!["Kubernetes", "TDT"]
        );
        let built_in = parse(DEVELOPER_TERMS).len();
        assert!(built_in > 100);
        // "Kubernetes" is already built in, so only "Acme Cloud" is added.
        assert_eq!(merged("Kubernetes\nAcme Cloud").len(), built_in + 1);
    }

    #[test]
    fn the_user_file_is_read_again_only_after_it_changes() {
        let dir = std::env::temp_dir().join(format!("tdt-dictionary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("dictionary.txt");
        std::fs::write(&path, "Acme Cloud\n").expect("write");
        let mut watch = Watch::default();
        let first = watch.changed_terms(Some(&path)).expect("first read");
        assert!(first.contains(&"Acme Cloud".to_string()));
        assert!(watch.changed_terms(Some(&path)).is_none());
        // A new modification time triggers a reload.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "Zeta Store\n").expect("rewrite");
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open");
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .expect("touch");
        let second = watch.changed_terms(Some(&path)).expect("reload");
        assert!(second.contains(&"Zeta Store".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
