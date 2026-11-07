//! Live reload: inotify on the config directory (not the file, because
//! editors save by writing a temporary file and renaming it over the old one).

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

use inotify::{Inotify, WatchMask};

/// Tells when `config.toml` may have changed. The fd is non-blocking, so a
/// `calloop` loop can wrap it in a `Generic` source and call
/// [`Watcher::changed`] when it is readable; simple callers use
/// [`Watcher::wait`].
#[derive(Debug)]
pub struct Watcher {
    inotify: Inotify,
    name: OsString,
    buf: Vec<u8>,
}

impl Watcher {
    /// Watch the file at `path`. Its directory is created if needed, so a
    /// config written later is still noticed.
    pub fn new(path: &Path) -> io::Result<Self> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file path"))?
            .to_owned();
        let inotify = Inotify::init()?;
        inotify.watches().add(
            dir,
            WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::CREATE | WatchMask::DELETE,
        )?;
        Ok(Watcher {
            inotify,
            name,
            buf: vec![0; 4096],
        })
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
                        hit |= event.name == Some(self.name.as_os_str());
                    }
                    if !any {
                        return Ok(hit);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(hit),
                Err(e) => return Err(e),
            }
        }
    }

    /// Block until the file has changed.
    pub fn wait(&mut self) -> io::Result<()> {
        loop {
            let mut fds = [rustix::event::PollFd::new(
                &self.inotify,
                rustix::event::PollFlags::IN,
            )];
            match rustix::event::poll(&mut fds, None) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
            if self.changed()? {
                return Ok(());
            }
        }
    }
}

impl AsFd for Watcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }
}
