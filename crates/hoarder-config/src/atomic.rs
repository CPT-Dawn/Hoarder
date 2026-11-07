//! Writing Hoarder's own files so nobody ever reads half of one.

use std::path::Path;

/// Replace `path` with `contents` atomically: write a sibling, then rename it
/// over the file, so a running process (or a concurrent run) sees the old
/// file or the new one, never a mix. Missing parent directories are created.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let written = std::fs::write(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_file_whole_and_leaves_nothing_else_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");

        write_atomic(&path, "a = 1\n").unwrap();
        write_atomic(&path, "b = 2\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b = 2\n");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.toml"]);
    }
}
