//! Per-user "start at login" on macOS via a LaunchAgent plist.

use std::fs;
use std::path::{Path, PathBuf};

const LABEL: &str = "com.tdt.app";
const PLIST_NAME: &str = "com.tdt.app.plist";

/// True when TDT has a LaunchAgent installed in `~/Library/LaunchAgents`.
pub fn is_enabled() -> bool {
    plist_path().is_some_and(|path| path.is_file())
}

/// Keep the LaunchAgent pointing at the current binary after a move or update.
pub fn refresh_if_enabled() {
    let Some(plist) = plist_path() else {
        return;
    };
    if !plist.is_file() {
        return;
    }
    let Ok(exe) = current_tdt_exe() else {
        return;
    };
    let expected = render_plist(&exe);
    if fs::read_to_string(&plist).ok().as_deref() != Some(expected.as_str()) {
        let _ = write_plist(&plist, &expected);
    }
}

/// Register or unregister TDT as a per-user login item.
pub fn set_enabled(enable: bool) -> Result<(), String> {
    let plist =
        plist_path().ok_or_else(|| "Could not locate ~/Library/LaunchAgents.".to_string())?;
    if enable {
        let exe = current_tdt_exe()?;
        let body = render_plist(&exe);
        write_plist(&plist, &body)
    } else {
        match fs::remove_file(&plist) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not remove the login item: {error}")),
        }
    }
}

fn plist_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(PLIST_NAME),
    )
}

fn current_tdt_exe() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("Could not locate TDT binary: {error}"))?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

fn write_plist(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create LaunchAgents directory: {error}"))?;
    }
    fs::write(path, body).map_err(|error| format!("Could not write login item: {error}"))
}

fn render_plist(exe: &Path) -> String {
    let workdir = exe.parent().unwrap_or(exe);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>{}</string>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy()),
        xml_escape(&workdir.to_string_lossy()),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plist_escapes_xml_and_sets_run_at_load() {
        let plist = render_plist(Path::new(
            "/Applications/TDT & Friends/TDT.app/Contents/MacOS/TDT",
        ));
        assert!(plist.contains("<string>com.tdt.app</string>"));
        assert!(plist.contains("TDT &amp; Friends"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
    }
}
