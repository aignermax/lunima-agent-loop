//! Locates the loop's root folder (agent-loop.json, state/, logs/, prompts/) and the
//! `lunima-agent-loop` CLI — with the same rules the C# loop uses.

use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "agent-loop.json";
const EXAMPLE_FILE: &str = "agent-loop.example.json";
const CLI_NAME: &str = if cfg!(windows) { "lunima-agent-loop.exe" } else { "lunima-agent-loop" };

/// Walks up from `start` until a folder contains the config (or its example).
pub fn find_root_from(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(CONFIG_FILE).is_file() || d.join(EXAMPLE_FILE).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Per-user data folder the installed (MSI) loop falls back to.
pub fn default_data_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
    }?;
    Some(base.join("lunima-agent-loop"))
}

/// Root resolution order: explicit argument → cwd → exe folder → per-user data folder.
pub fn resolve_root(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    let cwd = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    cwd.as_deref()
        .and_then(find_root_from)
        .or_else(|| exe_dir.as_deref().and_then(find_root_from))
        .or_else(default_data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The CLI the scheduled task runs: `<root>/publish/`, next to this exe, `<root>/`, then PATH.
pub fn find_cli(root: &Path) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let candidates = [
        Some(root.join("publish").join(CLI_NAME)),
        exe_dir.map(|d| d.join(CLI_NAME)),
        Some(root.join(CLI_NAME)),
    ];
    if let Some(found) = candidates.into_iter().flatten().find(|p| p.is_file()) {
        return Some(found);
    }
    find_on_path(CLI_NAME)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_root_in_parent_folder() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(CONFIG_FILE), "{}").unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_root_from(&nested).as_deref(), Some(tmp.path()));
    }

    #[test]
    fn example_config_also_marks_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(EXAMPLE_FILE), "{}").unwrap();
        assert_eq!(find_root_from(tmp.path()).as_deref(), Some(tmp.path()));
    }

    #[test]
    fn explicit_root_wins() {
        let p = PathBuf::from("C:/somewhere");
        assert_eq!(resolve_root(Some(p.clone())), p);
    }

    #[test]
    fn cli_in_publish_folder_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("publish")).unwrap();
        std::fs::write(tmp.path().join("publish").join(CLI_NAME), "").unwrap();
        assert_eq!(find_cli(tmp.path()), Some(tmp.path().join("publish").join(CLI_NAME)));
    }
}
