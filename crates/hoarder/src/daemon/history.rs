//! The Daemon's History (PLAN.md §5.3): the Store lives on its own worker
//! thread, so SQLite and blob writes never stall the event loop. The loop
//! sends [`Job`]s; results come back as [`Done`] on a calloop channel.

use std::fs::File;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use calloop::LoopHandle;
use calloop::channel::{Event, channel};
use hoarder_ipc::{ClipSummary, ErrorCode, Failure, Got, Value};
use hoarder_store::{Clip, Store};

use super::{Daemon, Representation};

/// Work for the History thread. `client` and `id` say whom to answer.
#[derive(Debug)]
pub(super) enum Job {
    Record {
        reprs: Vec<Representation>,
        app: Option<String>,
    },
    List {
        client: u64,
        id: u64,
        limit: Option<u32>,
    },
    Get {
        client: u64,
        id: u64,
        clip: i64,
        mime: Option<String>,
        pipe: OwnedFd,
    },
    Restore {
        client: u64,
        id: u64,
        clip: i64,
    },
    Delete {
        client: u64,
        id: u64,
        clip: i64,
    },
}

/// What the History thread hands back to the loop.
#[derive(Debug)]
pub(super) enum Done {
    Reply {
        client: u64,
        id: u64,
        result: Result<Value, Failure>,
    },
    /// A Clip to make the Selection.
    Restore {
        client: u64,
        id: u64,
        clip: ClipSummary,
        reprs: Vec<Representation>,
    },
}

/// The loop's handle on the History thread.
#[derive(Debug)]
pub(super) struct History {
    jobs: Result<mpsc::Sender<Job>, String>,
}

/// `$XDG_DATA_HOME/hoarder`, or `~/.local/share/hoarder`.
fn dir() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data_home.join("hoarder"))
}

impl History {
    /// Open the Store and start its thread. A Store that cannot open leaves
    /// the Daemon running without History.
    pub(super) fn start(handle: &LoopHandle<'static, Daemon>) -> History {
        let store = dir()
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_owned())
            .and_then(|dir| {
                Store::open(&dir).map_err(|e| format!("cannot open {}: {e}", dir.display()))
            });
        let store = match store {
            Ok(store) => store,
            Err(e) => {
                eprintln!("hoarder: History is off: {e}");
                return History { jobs: Err(e) };
            }
        };
        let (done_tx, done_rx) = channel::<Done>();
        let inserted = handle.insert_source(done_rx, |event, _, daemon: &mut Daemon| {
            if let Event::Msg(done) = event {
                daemon.history_done(done);
            }
        });
        if let Err(e) = inserted {
            let e = format!("event loop: {}", e.error);
            eprintln!("hoarder: History is off: {e}");
            return History { jobs: Err(e) };
        }
        let (jobs, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("history".into())
            .spawn(move || worker(store, rx, done_tx))
            .map(|_| History { jobs: Ok(jobs) })
            .unwrap_or_else(|e| History {
                jobs: Err(format!("cannot start the History thread: {e}")),
            })
    }

    /// Queue `job`; if History is off, the failure to answer with.
    pub(super) fn send(&self, job: Job) -> Result<(), Failure> {
        let unavailable = |why: &str| Failure {
            code: ErrorCode::Failed,
            message: format!("History is unavailable: {why}"),
        };
        match &self.jobs {
            Ok(jobs) => jobs
                .send(job)
                .map_err(|_| unavailable("its thread has stopped")),
            Err(e) => Err(unavailable(e)),
        }
    }
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn worker(mut store: Store, jobs: mpsc::Receiver<Job>, done: calloop::channel::Sender<Done>) {
    for job in jobs {
        let reply = |client, id, result| Done::Reply { client, id, result };
        let out = match job {
            Job::Record { reprs, app } => {
                let reprs: Vec<(&str, &[u8])> =
                    reprs.iter().map(|(m, d)| (m.as_str(), &d[..])).collect();
                if let Err(e) = store.record(&reprs, app.as_deref(), now()) {
                    eprintln!("hoarder: cannot record a Clip: {e}");
                }
                continue;
            }
            Job::List { client, id, limit } => {
                let limit = limit.map_or(usize::MAX, |l| l as usize);
                let result = store
                    .list(limit)
                    .map(|clips| to_value(&clips.iter().map(summary).collect::<Vec<_>>()))
                    .map_err(failed);
                reply(client, id, result)
            }
            Job::Get {
                client,
                id,
                clip,
                mime,
                pipe,
            } => reply(client, id, get(&store, clip, mime, pipe)),
            Job::Restore { client, id, clip } => match restore(&mut store, clip) {
                Ok((clip, reprs)) => Done::Restore {
                    client,
                    id,
                    clip,
                    reprs,
                },
                Err(f) => reply(client, id, Err(f)),
            },
            Job::Delete { client, id, clip } => {
                let result = match store.delete(clip) {
                    Ok(true) => Ok(Value::Null),
                    Ok(false) => Err(not_found(clip)),
                    Err(e) => Err(failed(e)),
                };
                reply(client, id, result)
            }
        };
        if done.send(out).is_err() {
            return;
        }
    }
}

/// Answer `get` and stream the data into `pipe` on a thread of its own, so
/// a slow reader stalls nobody.
fn get(store: &Store, clip: i64, mime: Option<String>, pipe: OwnedFd) -> Result<Value, Failure> {
    let meta = store.clip(clip).map_err(failed)?.ok_or(not_found(clip))?;
    let reprs = store
        .representations(clip)
        .map_err(failed)?
        .ok_or(not_found(clip))?;
    let found = match &mime {
        Some(m) => reprs.into_iter().find(|(t, _)| t == m),
        None => reprs.into_iter().next(),
    };
    let Some((mime, data)) = found else {
        return Err(Failure {
            code: ErrorCode::NotFound,
            message: format!(
                "Clip {clip} has no {} representation",
                mime.unwrap_or_default()
            ),
        });
    };
    let got = Got {
        clip: summary(&meta),
        mime,
        size: data.len() as u64,
    };
    std::thread::spawn(move || {
        // A reader that hangs up early is its own business.
        let _ = File::from(pipe).write_all(&data);
    });
    Ok(to_value(&got))
}

fn restore(store: &mut Store, clip: i64) -> Result<(ClipSummary, Vec<Representation>), Failure> {
    let reprs = store
        .representations(clip)
        .map_err(failed)?
        .ok_or(not_found(clip))?;
    if reprs.is_empty() {
        return Err(Failure {
            code: ErrorCode::Failed,
            message: format!("Clip {clip} has nothing to restore"),
        });
    }
    store.touch(clip, now()).map_err(failed)?;
    let meta = store.clip(clip).map_err(failed)?.ok_or(not_found(clip))?;
    let reprs = reprs
        .into_iter()
        .map(|(m, d)| (m, Arc::from(d.into_boxed_slice())))
        .collect();
    Ok((summary(&meta), reprs))
}

pub(super) fn summary(clip: &Clip) -> ClipSummary {
    let time = |ms: i64| {
        jiff::Timestamp::from_millisecond(ms)
            .map(|t| t.to_string())
            .unwrap_or_default()
    };
    ClipSummary {
        id: clip.id,
        kind: clip.kind.to_string(),
        preview: clip.preview.clone(),
        app: clip.source_app.clone(),
        at: time(clip.last_used_at),
        created: time(clip.created_at),
        pinned: clip.pinned,
        size: clip.size,
        types: clip.types.clone(),
    }
}

fn not_found(clip: i64) -> Failure {
    Failure {
        code: ErrorCode::NotFound,
        message: format!("no Clip {clip} in History"),
    }
}

fn failed(e: hoarder_store::Error) -> Failure {
    Failure {
        code: ErrorCode::Failed,
        message: e.to_string(),
    }
}

pub(super) fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
