//! The Daemon's History (PLAN.md §5.3): the Store lives on its own worker
//! thread, so SQLite and blob writes never stall the event loop. The loop
//! sends [`Job`]s; results come back as [`Done`] on a calloop channel.
//!
//! The worker also opens the Store, which under `history.encrypt` means a
//! trip to the keyring. If that fails (no Secret Service, a locked
//! keyring), History is unavailable with the reason, nothing is recorded,
//! and every later job tries again.

use std::fs::File;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use calloop::LoopHandle;
use calloop::channel::{self as loop_channel, channel};
use hoarder_ipc::{Cleared, ClipSummary, ErrorCode, Event, Failure, Got, Value};
use hoarder_store::retention::Limits;
use hoarder_store::{Clip, Store};

use super::keyring;
use super::{Daemon, ReplyTo, Representation};

/// Work for the History thread. `to` says whom to answer.
#[derive(Debug)]
pub(super) enum Job {
    /// Record a Clip, then apply retention.
    Record {
        reprs: Vec<Representation>,
        app: Option<String>,
        limits: Limits,
    },
    /// Apply retention (the hourly run, or after a config change).
    Retain {
        limits: Limits,
    },
    List {
        to: ReplyTo,
        limit: Option<u32>,
        /// Concealed Clips to list among History's, with when each was
        /// copied (ms since the Unix epoch), newest first.
        extra: Vec<(i64, ClipSummary)>,
    },
    Search {
        to: ReplyTo,
        query: String,
        limit: Option<u32>,
    },
    Get {
        to: ReplyTo,
        clip: i64,
        mime: Option<String>,
        pipe: OwnedFd,
    },
    Thumb {
        to: ReplyTo,
        clip: i64,
        pipe: OwnedFd,
    },
    Restore {
        to: ReplyTo,
        clip: i64,
        /// Plain text only.
        plain: bool,
    },
    Delete {
        to: ReplyTo,
        clip: i64,
    },
    /// `history.encrypt` changed: migrate the History to match.
    Encrypt(bool),
    Pin {
        to: ReplyTo,
        clip: i64,
        pinned: bool,
    },
    Clear {
        to: ReplyTo,
        pinned: bool,
    },
}

/// The retention limits `history` sets; 0 means no limit.
pub(super) fn limits(history: &hoarder_config::History) -> Limits {
    const MB: u64 = 1024 * 1024;
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let some = |v: u32| (v > 0).then_some(u64::from(v));
    Limits {
        max_clips: some(history.max_clips.get()).map(|v| v as usize),
        max_age_ms: some(history.max_age_days.get()).map(|d| d as i64 * DAY_MS),
        max_clip_bytes: some(history.max_clip_mb.get()).map(|v| v * MB),
        max_total_bytes: some(history.max_total_mb.get()).map(|v| v * MB),
    }
}

/// What the History thread hands back to the loop.
#[derive(Debug)]
pub(super) enum Done {
    Reply {
        to: ReplyTo,
        result: Result<Value, Failure>,
    },
    /// A Clip to make the Selection.
    Restore {
        to: ReplyTo,
        clip: ClipSummary,
        reprs: Vec<Representation>,
    },
    /// History changed: tell the subscribers.
    Changed(Vec<Event>),
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
    /// Start the History thread, which opens the Store (encrypted or not,
    /// per `encrypt`). A History that cannot open answers every request
    /// with the reason; the Daemon runs on without it.
    pub(super) fn start(handle: &LoopHandle<'static, Daemon>, encrypt: bool) -> History {
        let Some(dir) = dir() else {
            let e = "neither XDG_DATA_HOME nor HOME is set".to_owned();
            eprintln!("hoarder: History is off: {e}");
            return History { jobs: Err(e) };
        };
        let (done_tx, done_rx) = channel::<Done>();
        let inserted = handle.insert_source(done_rx, |event, _, daemon: &mut Daemon| {
            if let loop_channel::Event::Msg(done) = event {
                daemon.history_done(done);
            }
        });
        if let Err(e) = inserted {
            let e = format!("event loop: {}", e.error);
            eprintln!("hoarder: History is off: {e}");
            return History { jobs: Err(e) };
        }
        let (jobs, rx) = mpsc::channel();
        let vault = Vault {
            dir,
            encrypt,
            store: None,
            failure: None,
        };
        std::thread::Builder::new()
            .name("history".into())
            .spawn(move || worker(vault, rx, done_tx))
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

/// The Store, opened on demand to match `history.encrypt`.
#[derive(Debug)]
struct Vault {
    dir: PathBuf,
    encrypt: bool,
    store: Option<Store>,
    /// The last reason the Store would not open, logged once.
    failure: Option<String>,
}

impl Vault {
    /// The open Store, opening it (again) if need be.
    fn store(&mut self) -> Result<&mut Store, Failure> {
        if self.store.is_none() {
            match open(&self.dir, self.encrypt) {
                Ok(store) => {
                    if self.failure.take().is_some() {
                        eprintln!("hoarder: History is available again");
                    }
                    self.store = Some(store);
                }
                Err(e) => {
                    if self.failure.as_ref() != Some(&e) {
                        eprintln!("hoarder: History is unavailable: {e}");
                        self.failure = Some(e.clone());
                    }
                    return Err(Failure {
                        code: ErrorCode::Failed,
                        message: format!("History is unavailable: {e}"),
                    });
                }
            }
        }
        Ok(self.store.as_mut().expect("just opened"))
    }

    /// Follow a change of `history.encrypt`: reopen, migrating on the way.
    fn set_encrypt(&mut self, encrypt: bool) {
        if self.encrypt != encrypt || self.store.is_none() {
            self.encrypt = encrypt;
            self.store = None;
            let _ = self.store();
        }
    }
}

/// Open the Store in `dir` and bring it to `encrypt`: unlock an encrypted
/// History with the keyring's key, then encrypt or decrypt it as asked.
fn open(dir: &Path, encrypt: bool) -> Result<Store, String> {
    let mut store = Store::open(dir).map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    if store.is_encrypted() {
        let key = keyring::history_key(false)?.ok_or_else(|| {
            format!(
                "the History key is missing from the keyring, so the encrypted History in {} \
                 cannot be read; restore the key, or delete that directory to start afresh",
                dir.display()
            )
        })?;
        store.unlock(key).map_err(|e| match e {
            hoarder_store::Error::WrongKey => format!(
                "the History key in the keyring does not match the History in {}",
                dir.display()
            ),
            e => e.to_string(),
        })?;
        if !encrypt {
            store
                .decrypt()
                .map_err(|e| format!("cannot decrypt History: {e}"))?;
            eprintln!("hoarder: History decrypted");
        }
    } else if encrypt {
        let key = keyring::history_key(true)?.ok_or("the keyring did not keep the History key")?;
        store
            .encrypt(key)
            .map_err(|e| format!("cannot encrypt History: {e}"))?;
        eprintln!("hoarder: History encrypted");
    }
    Ok(store)
}

fn worker(mut vault: Vault, jobs: mpsc::Receiver<Job>, done: calloop::channel::Sender<Done>) {
    // Open up front, so any keyring trouble is logged at start.
    let _ = vault.store();
    for job in jobs {
        let reply = |to, result| Some(Done::Reply { to, result });
        // What subscribers hear of this job, sent before its answer.
        let mut events = Vec::new();
        let out = match job {
            Job::Encrypt(encrypt) => {
                vault.set_encrypt(encrypt);
                continue;
            }
            Job::Record { reprs, app, limits } => {
                // Unavailable (say, a locked keyring): drop it, never store it plain.
                let Ok(store) = vault.store() else {
                    continue;
                };
                let reprs: Vec<(&str, &[u8])> =
                    reprs.iter().map(|(m, d)| (m.as_str(), &d[..])).collect();
                match store.record(&reprs, app.as_deref(), now()) {
                    Ok(recorded) => {
                        if recorded.new {
                            add_thumb(store, recorded.id, &reprs);
                        }
                        if let Ok(Some(clip)) = store.clip(recorded.id) {
                            events.push(Event::ClipAdded {
                                clip: summary(&clip),
                            });
                        }
                        events.extend(retain(store, &limits));
                    }
                    Err(e) => eprintln!("hoarder: cannot record a Clip: {e}"),
                }
                None
            }
            Job::Thumb { to, clip, pipe } => {
                reply(to, vault.store().and_then(|store| thumb(store, clip, pipe)))
            }
            Job::Retain { limits } => {
                if let Ok(store) = vault.store() {
                    events.extend(retain(store, &limits));
                }
                None
            }
            Job::List { to, limit, extra } => {
                let limit = limit.map_or(usize::MAX, |l| l as usize);
                let result = vault.store().and_then(|store| {
                    store
                        .list(limit)
                        .map(|clips| to_value(&merge(&clips, extra, limit)))
                        .map_err(failed)
                });
                reply(to, result)
            }
            Job::Search { to, query, limit } => {
                let limit = limit.map_or(usize::MAX, |l| l as usize);
                let result = vault.store().and_then(|store| {
                    store
                        .search(&query, limit)
                        .map(|clips| to_value(&clips.iter().map(summary).collect::<Vec<_>>()))
                        .map_err(failed)
                });
                reply(to, result)
            }
            Job::Get {
                to,
                clip,
                mime,
                pipe,
            } => reply(
                to,
                vault.store().and_then(|store| get(store, clip, mime, pipe)),
            ),
            Job::Restore { to, clip, plain } => {
                match vault.store().and_then(|store| restore(store, clip, plain)) {
                    Ok((clip, reprs)) => Some(Done::Restore { to, clip, reprs }),
                    Err(f) => reply(to, Err(f)),
                }
            }
            Job::Delete { to, clip } => {
                let result = vault.store().and_then(|store| match store.delete(clip) {
                    Ok(true) => {
                        events.push(Event::ClipRemoved { clip });
                        Ok(Value::Null)
                    }
                    Ok(false) => Err(not_found(clip)),
                    Err(e) => Err(failed(e)),
                });
                reply(to, result)
            }
            Job::Pin { to, clip, pinned } => {
                let result = vault
                    .store()
                    .and_then(|store| match store.pin(clip, pinned) {
                        Ok(true) => store
                            .clip(clip)
                            .map_err(failed)
                            .and_then(|c| c.ok_or(not_found(clip)))
                            .map(|c| to_value(&summary(&c))),
                        Ok(false) => Err(not_found(clip)),
                        Err(e) => Err(failed(e)),
                    });
                reply(to, result)
            }
            Job::Clear { to, pinned } => {
                let result = vault.store().and_then(|store| clear(store, pinned));
                let result = result.map(|gone| {
                    let deleted = gone.len() as u64;
                    events.extend(gone.into_iter().map(|clip| Event::ClipRemoved { clip }));
                    to_value(&Cleared { deleted })
                });
                reply(to, result)
            }
        };
        let changed = (!events.is_empty()).then_some(Done::Changed(events));
        for done_ in changed.into_iter().chain(out) {
            if done.send(done_).is_err() {
                return;
            }
        }
    }
}

/// Empty History (pinned Clips too if `pinned`); the ids that went.
fn clear(store: &mut Store, pinned: bool) -> Result<Vec<i64>, Failure> {
    // The one thread with the Store, so nothing comes or goes in between.
    let ids: Vec<i64> = store
        .list(usize::MAX)
        .map_err(failed)?
        .iter()
        .filter(|c| pinned || !c.pinned)
        .map(|c| c.id)
        .collect();
    store.clear(pinned).map_err(failed)?;
    Ok(ids)
}

/// Apply `limits`; a `clip_removed` for each Clip evicted.
fn retain(store: &mut Store, limits: &Limits) -> Vec<Event> {
    match store.retain(limits, now()) {
        Ok(gone) => gone
            .into_iter()
            .map(|clip| Event::ClipRemoved { clip })
            .collect(),
        Err(e) => {
            eprintln!("hoarder: cannot apply History limits: {e}");
            Vec::new()
        }
    }
}

/// History's `clips` and the Concealed `extra`, both newest first, merged
/// most recently used first and cut to `limit`.
fn merge(clips: &[Clip], extra: Vec<(i64, ClipSummary)>, limit: usize) -> Vec<ClipSummary> {
    let mut out = Vec::with_capacity(clips.len() + extra.len());
    let mut extra = extra.into_iter().peekable();
    for clip in clips {
        while let Some((_, concealed)) = extra.next_if(|(at, _)| *at > clip.last_used_at) {
            out.push(concealed);
        }
        out.push(summary(clip));
    }
    out.extend(extra.map(|(_, c)| c));
    out.truncate(limit);
    out
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

/// Thumbnails fit in a square this many pixels on a side (PLAN.md §5.3).
const THUMB_SIZE: u32 = 256;

/// Give a new image Clip its thumbnail, made from its PNG (or its first
/// image type). An image that does not decode simply has none.
fn add_thumb(store: &mut Store, clip: i64, reprs: &[(&str, &[u8])]) {
    let image = reprs
        .iter()
        .find(|(m, _)| *m == "image/png")
        .or_else(|| reprs.iter().find(|(m, _)| m.starts_with("image/")));
    let Some((_, data)) = image else { return };
    let Ok(decoded) = image::load_from_memory(data) else {
        return;
    };
    let small =
        image::DynamicImage::ImageRgba8(decoded.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgba8());
    let mut png = Vec::new();
    if small
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .is_ok()
        && let Err(e) = store.set_thumb(clip, &png)
    {
        eprintln!("hoarder: cannot keep a thumbnail: {e}");
    }
}

/// Answer `thumb`, streaming the PNG into `pipe` off-thread.
fn thumb(store: &Store, clip: i64, pipe: OwnedFd) -> Result<Value, Failure> {
    let meta = store.clip(clip).map_err(failed)?.ok_or(not_found(clip))?;
    let png = store.thumb(clip).map_err(failed)?.ok_or_else(|| Failure {
        code: ErrorCode::NotFound,
        message: format!("Clip {clip} has no thumbnail"),
    })?;
    let got = Got {
        clip: summary(&meta),
        mime: "image/png".into(),
        size: png.len() as u64,
    };
    std::thread::spawn(move || {
        let _ = File::from(pipe).write_all(&png);
    });
    Ok(to_value(&got))
}

fn restore(
    store: &mut Store,
    clip: i64,
    plain: bool,
) -> Result<(ClipSummary, Vec<Representation>), Failure> {
    let mut reprs = store
        .representations(clip)
        .map_err(failed)?
        .ok_or(not_found(clip))?;
    if plain {
        reprs.retain(|(m, _)| super::is_plain_text(m));
        if reprs.is_empty() {
            return Err(no_plain_text(clip));
        }
    }
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

/// A Store kind as the wire names it.
fn kind(kind: hoarder_store::Kind) -> hoarder_ipc::Kind {
    match kind {
        hoarder_store::Kind::Text => hoarder_ipc::Kind::Text,
        hoarder_store::Kind::Image => hoarder_ipc::Kind::Image,
        hoarder_store::Kind::Files => hoarder_ipc::Kind::Files,
        hoarder_store::Kind::Other => hoarder_ipc::Kind::Other,
    }
}

pub(super) fn summary(clip: &Clip) -> ClipSummary {
    let time = |ms: i64| {
        jiff::Timestamp::from_millisecond(ms)
            .map(|t| t.to_string())
            .unwrap_or_default()
    };
    ClipSummary {
        id: clip.id,
        kind: kind(clip.kind),
        preview: clip.preview.clone(),
        app: clip.source_app.clone(),
        at: time(clip.last_used_at),
        created: time(clip.created_at),
        pinned: clip.pinned,
        size: clip.size,
        types: clip.types.clone(),
        sensitivity: Default::default(),
        expires: None,
    }
}

pub(super) fn no_plain_text(clip: i64) -> Failure {
    Failure {
        code: ErrorCode::NotFound,
        message: format!("Clip {clip} has no plain text"),
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
