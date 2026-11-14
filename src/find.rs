//! Find the room's movie on this Mac by its exact file name, with Spotlight.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A Spotlight query for files named exactly `name`. Quotes, backslashes and
/// `*` (a wildcard in Spotlight) are escaped so the name is matched literally.
pub fn query(name: &str) -> String {
    let mut escaped = String::new();
    for c in name.chars() {
        if matches!(c, '"' | '\\' | '*') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    format!("kMDItemFSName == \"{escaped}\"")
}

/// The first file on this Mac named exactly `name`, if Spotlight knows one.
pub fn find_by_name(name: &str) -> Option<PathBuf> {
    let name = crate::share::safe_file_name(name)?;
    let out = Command::new("/usr/bin/mdfind").arg(query(&name)).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(PathBuf::from)
        .find(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(name.as_str()))
}

/// "Downloads › Movies" for a found file's folder, relative to home.
pub fn describe_folder(path: &Path) -> String {
    let dir = path.parent().unwrap_or(path);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let shown = match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rel) if rel.as_os_str().is_empty() => return "your home folder".into(),
        Some(rel) => rel.to_path_buf(),
        None => dir.to_path_buf(),
    };
    shown
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter(|c| *c != "/")
        .collect::<Vec<_>>()
        .join(" › ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_matched_literally() {
        assert_eq!(query("Movie (2026).mkv"), r#"kMDItemFSName == "Movie (2026).mkv""#);
        assert_eq!(query(r#"a "b" *c\.mkv"#), r#"kMDItemFSName == "a \"b\" \*c\\.mkv""#);
    }

    #[test]
    fn folders_read_like_finder() {
        let home = std::env::var("HOME").unwrap();
        let p = PathBuf::from(format!("{home}/Downloads/Movies/x.mkv"));
        assert_eq!(describe_folder(&p), "Downloads › Movies");
        assert_eq!(describe_folder(Path::new("/Volumes/SSD/Films/x.mkv")), "Volumes › SSD › Films");
    }
}
