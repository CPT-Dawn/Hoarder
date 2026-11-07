//! Live reload: inotify on the config directory (not the file, because
//! editors save by writing a temporary file and renaming it over the old one).

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::time::Duration;

use inotify::{Inotify, WatchMask};
use rustix::event::{PollFd, PollFlags};

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
            // MODIFY too, so a writer still busy in place (truncate, then
            // write) keeps the file from looking settled.
            WatchMask::CLOSE_WRITE
                | WatchMask::MODIFY
                | WatchMask::MOVED_TO
                | WatchMask::CREATE
                | WatchMask::DELETE,
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
