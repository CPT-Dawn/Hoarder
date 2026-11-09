//! Live reload: inotify on the config directory (not the file, because
//! editors save by writing a temporary file and renaming it over the old one),
//! plus the directories a symlinked config points into.

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use rustix::event::{PollFd, PollFlags};

/// Tells when `config.toml` may have changed. The fd is non-blocking, so a
/// `calloop` loop can wrap it in a `Generic` source and call
/// [`Watcher::changed`] when it is readable; simple callers use
/// [`Watcher::wait`].
///
/// If the file is a symlink (dotfiles managed by stow or chezmoi), the
/// directory of every link in the chain is watched too, since editors write
/// into the target's directory; the chain is re-resolved whenever it changes.
/// A watched directory that is removed and re-created is watched again.
#[derive(Debug)]
pub struct Watcher {
    inotify: Inotify,
    path: PathBuf,
    /// Directory and file name of every link in the chain, `path` first.
    files: Vec<(PathBuf, OsString)>,
    watches: Vec<(WatchDescriptor, PathBuf)>,
    buf: Vec<u8>,
}

// MODIFY too, so a writer still busy in place (truncate, then write) keeps
// the file from looking settled.
const MASK: WatchMask = WatchMask::CLOSE_WRITE
    .union(WatchMask::MODIFY)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::CREATE)
    .union(WatchMask::DELETE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/// `path` followed by each symlink target in turn (a dangling link included).
fn chain(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    for _ in 0..40 {
        let last = out.last().expect("never empty");
        let Ok(target) = std::fs::read_link(last) else {
            break;
        };
        let next = match last.parent() {
            Some(dir) => dir.join(target),
            None => target,
        };
        out.push(next);
    }
    out
}

impl Watcher {
    /// Watch the file at `path`. Its directory is created if needed, so a
    /// config written later is still noticed.
    pub fn new(path: &Path) -> io::Result<Self> {
        if path.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a file path",
            ));
        }
        let mut watcher = Watcher {
            inotify: Inotify::init()?,
            path: path.to_path_buf(),
            files: Vec::new(),
            watches: Vec::new(),
            buf: vec![0; 4096],
        };
        watcher.refresh()?;
        Ok(watcher)
    }

    /// Re-resolve the symlink chain and (re-)add the watches it needs.
    fn refresh(&mut self) -> io::Result<()> {
        let dir = |p: &Path| match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        std::fs::create_dir_all(dir(&self.path))?;
        self.files = chain(&self.path)
            .into_iter()
            .filter_map(|p| Some((dir(&p), p.file_name()?.to_owned())))
            .collect();
        let mut watches: Vec<(WatchDescriptor, PathBuf)> = Vec::new();
        for (i, (d, _)) in self.files.iter().enumerate() {
            if watches.iter().any(|(_, w)| w == d) {
                continue;
            }
            match self.inotify.watches().add(d, MASK) {
                Ok(wd) => watches.push((wd, d.clone())),
                // Only the config's own directory is mandatory.
                Err(e) if i == 0 => return Err(e),
                Err(_) => {}
            }
        }
        for (wd, _) in &self.watches {
            if !watches.iter().any(|(w, _)| w == wd) {
                let _ = self.inotify.watches().remove(wd.clone());
            }
        }
        self.watches = watches;
        Ok(())
    }

    /// Drain pending events without blocking; true if any was about the file.
    pub fn changed(&mut self) -> io::Result<bool> {
        let mut hit = false;
        loop {
            match self.inotify.read_events(&mut self.buf) {
                Ok(events) => {
                    let mut any = false;
                    for event in events {
                        any = true;
                        let Some((_, dir)) = self.watches.iter().find(|(w, _)| *w == event.wd)
                        else {
                            continue;
                        };
                        if event.mask.intersects(
                            EventMask::IGNORED | EventMask::DELETE_SELF | EventMask::MOVE_SELF,
                        ) {
                            // A watched directory went away: look again.
                            hit = true;
                        } else if let Some(name) = event.name {
                            hit |= self.files.iter().any(|(d, n)| d == dir && n == name);
                        }
                    }
                    if !any {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        if hit {
            // Best effort: a failed re-watch must not hide the change.
            let _ = self.refresh();
        }
        Ok(hit)
    }

    /// Block until the file has changed and then been quiet for
    /// [`SETTLE`], so a writer that truncates and rewrites in place is not
    /// caught halfway.
    pub fn wait(&mut self) -> io::Result<()> {
        while !self.poll(None)? {}
        self.settle()
    }

    /// Block until the file has been quiet for [`SETTLE`]. Call it after
    /// [`Watcher::changed`] reports that the file moved under a read.
    pub fn settle(&mut self) -> io::Result<()> {
        while self.poll(Some(SETTLE))? {}
        Ok(())
    }

    /// Wait up to `timeout` (forever with `None`) for events; true if any was
    /// about the file.
    fn poll(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        let timeout = timeout.map(|t| rustix::event::Timespec {
            tv_sec: t.as_secs() as _,
            tv_nsec: t.subsec_nanos().into(),
        });
        loop {
            let mut fds = [PollFd::new(&self.inotify, PollFlags::IN)];
            match rustix::event::poll(&mut fds, timeout.as_ref()) {
                Ok(0) => return Ok(false),
                Ok(_) => return self.changed(),
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// How long the file must be left alone before [`Watcher::wait`] returns.
/// Event loops using [`Watcher::changed`] should debounce by the same amount.
pub const SETTLE: Duration = Duration::from_millis(50);

impl AsFd for Watcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll until a change is reported (or give up after a second).
    fn sees_change(w: &mut Watcher) -> bool {
        for _ in 0..20 {
            if w.poll(Some(Duration::from_millis(50))).unwrap() {
                return true;
            }
        }
        false
    }

    fn linked(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let dots = tmp.join("dotfiles");
        let conf = tmp.join("config/hoarder");
        std::fs::create_dir_all(&dots).unwrap();
        std::fs::create_dir_all(&conf).unwrap();
        let target = dots.join("config.toml");
        std::fs::write(&target, "a = 1\n").unwrap();
        let link = conf.join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        (link, target)
    }

    #[test]
    fn symlinked_config_rewritten_in_place_is_noticed() {
        let tmp = tempfile::tempdir().unwrap();
        let (link, target) = linked(tmp.path());
        let mut w = Watcher::new(&link).unwrap();
        std::fs::write(&target, "a = 2\n").unwrap();
        assert!(sees_change(&mut w));
    }

    #[test]
    fn symlinked_config_replaced_by_rename_is_noticed() {
        let tmp = tempfile::tempdir().unwrap();
        let (link, target) = linked(tmp.path());
        let mut w = Watcher::new(&link).unwrap();
        let temp = target.with_extension("swp");
        std::fs::write(&temp, "a = 2\n").unwrap();
        std::fs::rename(&temp, &target).unwrap();
        assert!(sees_change(&mut w));
    }

    #[test]
    fn retargeted_symlink_is_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let (link, _) = linked(tmp.path());
        let mut w = Watcher::new(&link).unwrap();
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("config.toml"), "a = 3\n").unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(other.join("config.toml"), &link).unwrap();
        assert!(sees_change(&mut w));
        std::fs::write(other.join("config.toml"), "a = 4\n").unwrap();
        assert!(sees_change(&mut w));
    }

    #[test]
    fn recreated_config_directory_is_watched_again() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("hoarder");
        let path = dir.join("config.toml");
        let mut w = Watcher::new(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        w.poll(Some(Duration::from_millis(100))).unwrap();
        w.changed().unwrap();
        std::fs::write(&path, "a = 1\n").unwrap();
        assert!(sees_change(&mut w));
    }
}
