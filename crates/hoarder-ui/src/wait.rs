//! Dispatching Wayland events with a deadline, for the waits that must not
//! hang if the compositor never answers (a frame callback on an output that
//! is off, say).

use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use wayland_client::EventQueue;

/// Dispatch `queue` until `done(state)` holds or `deadline` passes, whichever
/// comes first. Returns whether `done` held.
pub(crate) fn dispatch_until<D>(
    queue: &mut EventQueue<D>,
    state: &mut D,
    deadline: Instant,
    done: impl Fn(&D) -> bool,
) -> Result<bool, String> {
    loop {
        queue.dispatch_pending(state).map_err(|e| e.to_string())?;
        if done(state) {
            return Ok(true);
        }
        queue.flush().map_err(|e| e.to_string())?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        let ready = {
            let fd = guard.connection_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN)];
            let timeout = Timespec::try_from(left.min(Duration::from_secs(60)))
                .expect("a minute fits in a timespec");
            match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(n) => n > 0,
                Err(rustix::io::Errno::INTR) => false,
                Err(e) => return Err(e.to_string()),
            }
        };
        if ready {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}
