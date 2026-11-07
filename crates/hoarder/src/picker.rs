//! `hoarder picker`: History in a panel; the Clip picked there becomes the
//! clipboard again (PLAN.md §6).

use std::thread::JoinHandle;
use std::time::Instant;

use hoarder_ipc::{ClipSummary, Connection, Failure};
use hoarder_ui::OverlayError;
use hoarder_ui::picker::{self, Entry, Options};

use crate::Exit;

type Fail = (Exit, String);
type Listed = (
    Connection,
    std::io::Result<Result<Vec<ClipSummary>, Failure>>,
);

pub fn run(overrides: &[(String, String)]) -> Result<(), Fail> {
    let started = Instant::now();
    let mut daemon = Connection::connect().map_err(|_| {
        (
            Exit::Io,
            "the Daemon is not running, and History lives in it; start it with \
             `hoarder service enable`"
                .to_owned(),
        )
    })?;
    // History arrives while the panel and its fonts get ready.
    let listing = std::thread::spawn(move || {
        let clips = daemon.list(None);
        (daemon, clips)
    });
    let config = crate::config::effective(overrides)?;
    let options = Options {
        theme: hoarder_ui::Theme::load(&config.theme),
        started,
    };
    let mut history = Daemon::Listing(Some(listing));
    let picked = picker::run(&mut history, &options).map_err(|e| match e {
        OverlayError::Unsupported(m) => (Exit::Unsupported, m),
        OverlayError::Failed(m) => (Exit::Io, m),
    })?;
    let Some(clip) = picked else {
        return Err((Exit::Cancelled, "cancelled".into()));
    };
    let mut daemon = history.connection()?;
    match daemon.restore(clip) {
        Ok(Ok(_)) => {
            crate::autopaste::after_restore(&config.picker);
            Ok(())
        }
        Ok(Err(failure)) => Err((Exit::Io, failure.message)),
        Err(e) => Err((Exit::Io, format!("lost the Daemon: {e}"))),
    }
}

/// History as the Daemon serves it.
enum Daemon {
    /// The first `list` is on its way.
    Listing(Option<JoinHandle<Listed>>),
    Ready(Connection),
    Lost(String),
}

impl Daemon {
    fn connection(self) -> Result<Connection, Fail> {
        match self {
            Daemon::Ready(c) => Ok(c),
            Daemon::Lost(why) => Err((Exit::Io, why)),
            Daemon::Listing(_) => Err((Exit::Io, "History never arrived".into())),
        }
    }
}

impl picker::History for Daemon {
    fn clips(&mut self) -> Result<Vec<Entry>, String> {
        let Daemon::Listing(listing) = self else {
            return Err("History was already read".into());
        };
        let listing = listing.take().ok_or("History was already read")?;
        let (connection, clips) = listing
            .join()
            .map_err(|_| "reading History failed".to_owned())?;
        let clips = match clips {
            Ok(Ok(clips)) => clips,
            Ok(Err(failure)) => return Err(failure.message),
            Err(e) => return Err(format!("lost the Daemon: {e}")),
        };
        *self = Daemon::Ready(connection);
        Ok(clips
            .into_iter()
            .map(|c| Entry {
                id: c.id,
                kind: c.kind,
                preview: c.preview,
            })
            .collect())
    }

    fn search(&mut self, query: &str) -> Vec<i64> {
        let Daemon::Ready(connection) = self else {
            return Vec::new();
        };
        match connection.search(query, None) {
            Ok(Ok(clips)) => clips.into_iter().map(|c| c.id).collect(),
            Ok(Err(_)) => Vec::new(),
            Err(e) => {
                *self = Daemon::Lost(format!("lost the Daemon: {e}"));
                Vec::new()
            }
        }
    }
}
