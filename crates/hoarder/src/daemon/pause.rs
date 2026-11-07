//! Pause (PLAN.md §5.1): while Paused nothing goes to History, but
//! Persistence and Takeover go on. A timed Pause ends on a one-shot timer,
//! so an untimed one costs no wakeups.

use std::time::Duration;

use calloop::RegistrationToken;
use calloop::timer::{TimeoutAction, Timer};
use hoarder_ipc::{Recording, State};

use super::{Daemon, SELECTIONS};

/// The Daemon is Paused.
#[derive(Debug)]
pub(super) struct Pause {
    /// When a timed Pause ends.
    until: Option<jiff::Timestamp>,
    timer: Option<RegistrationToken>,
}

impl Daemon {
    /// Pause, for `secs` seconds or until resumed; a new Pause replaces any
    /// running one. A read in flight is not recorded either.
    pub(super) fn pause(&mut self, secs: Option<u64>) -> Recording {
        self.end_pause();
        let mut pause = Pause {
            until: None,
            timer: None,
        };
        if let Some(secs) = secs {
            let length = Duration::from_secs(secs);
            pause.until = jiff::SignedDuration::try_from(length)
                .ok()
                .and_then(|d| jiff::Timestamp::now().checked_add(d).ok());
            let timer = self.handle.insert_source(
                Timer::from_duration(length),
                |_, _, daemon: &mut Daemon| {
                    if let Some(pause) = &mut daemon.pause {
                        // This timer is done; dropping it is calloop's job.
                        pause.timer = None;
                    }
                    if daemon.end_pause() {
                        daemon.announce_state();
                    }
                    TimeoutAction::Drop
                },
            );
            match timer {
                Ok(token) => pause.timer = Some(token),
                Err(e) => eprintln!("hoarder: cannot time the Pause, so it lasts: {}", e.error),
            }
        }
        for which in SELECTIONS {
            self.slots[which as usize].record = false;
        }
        self.pause = Some(pause);
        self.announce_state();
        self.recording()
    }

    /// Record again.
    pub(super) fn resume(&mut self) -> Recording {
        if self.end_pause() {
            self.announce_state();
        }
        self.recording()
    }

    /// End any Pause; whether there was one.
    fn end_pause(&mut self) -> bool {
        match self.pause.take() {
            Some(Pause {
                timer: Some(token), ..
            }) => {
                self.handle.remove(token);
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Tell subscribers the recording state.
    fn announce_state(&mut self) {
        let event = hoarder_ipc::Event::State(self.recording());
        self.broadcast(&event);
    }

    pub(super) fn recording(&self) -> Recording {
        match &self.pause {
            None => Recording {
                state: State::Active,
                until: None,
            },
            Some(pause) => Recording {
                state: State::Paused,
                until: pause.until.map(|t| t.to_string()),
            },
        }
    }
}
