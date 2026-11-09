//! Writing Hoarder's own files so nobody ever reads half of one.

use std::path::Path;

/// Replace `path` with `contents` atomically: write a sibling, then rename it
/// over the file, so a running process (or a concurrent run) sees the old
/// file or the new one, never a mix. Missing parent directories are created.
///
/// A symlinked `path` (dotfiles managed by stow or chezmoi) stays a symlink:
/// the file it points to is the one replaced. The replacement keeps the old
/// file's permissions and is on disk before the rename.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write as _;

    let path = &resolve_symlinks(path)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        if let Ok(old) = std::fs::metadata(path) {
            file.set_permissions(old.permissions())?;
        }
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        if let Some(dir) = dir {
            // The rename itself, durable too. Best effort: not every
            // filesystem lets a directory be synced.
            let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
        }
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `path` with every symlink at its end followed, even a dangling one, so a
/// write lands on the link's target instead of replacing the link.
fn resolve_symlinks(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&path)?;
                path = match path.parent() {
                    Some(dir) => dir.join(target),
                    None => target,
                };
            }
            _ => return Ok(path),
        }
    }
    Err(std::io::Error::other(format!(
        "too many levels of symbolic links: {}",
        path.display()
    )))
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
    #[test]
    fn writes_through_a_symlink_and_keeps_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let dotfiles = dir.path().join("dotfiles/hoarder.toml");
        std::fs::create_dir_all(dotfiles.parent().unwrap()).unwrap();
        std::fs::write(&dotfiles, "a = 1\n").unwrap();
        std::fs::set_permissions(&dotfiles, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("config/config.toml");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&dotfiles, &link).unwrap();

        write_atomic(&link, "b = 2\n").unwrap();

        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&dotfiles).unwrap(), "b = 2\n");
        let mode = dotfiles.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
