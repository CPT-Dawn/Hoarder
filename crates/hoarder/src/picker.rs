//! `hoarder picker`: History in a panel; the Clip picked there becomes the
//! clipboard again (PLAN.md §6).

use std::collections::HashMap;
use std::io::Read as _;
use std::thread::JoinHandle;
use std::time::Instant;

use hoarder_ipc::{ClipSummary, Connection, Failure, Kind};
use hoarder_ui::Error as UiError;
use hoarder_ui::picker::{self, Clip, Detail, Options};

use crate::Exit;

type Fail = (Exit, String);

/// The most text the Picker reads of one Clip: more than it can show.
const MAX_TEXT: u64 = 64 * 1024;

/// The biggest image the preview pane decodes.
const MAX_IMAGE: u64 = 64 * 1024 * 1024;
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
        // Concealed Clips too: masked, their text only on Alt.
        let clips = daemon.list_with(None, true);
        (daemon, clips)
    });
    let config = crate::config::effective(overrides)?;
    let options = Options {
        theme: hoarder_ui::Theme::load(&config.theme),
        started,
    };
    let mut history = Daemon::Listing(Some(listing));
    let picked = picker::run(&mut history, &options).map_err(|e| match e {
        UiError::Unsupported(m) => (Exit::Unsupported, m),
        UiError::Failed(m) => (Exit::Io, m),
    })?;
    let Some(pick) = picked else {
        return Err((Exit::Cancelled, "cancelled".into()));
    };
    let mut daemon = history.connection()?;
    match daemon.restore_as(pick.id, pick.plain) {
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
    /// With every Clip's MIME types, by id.
    Ready(Connection, HashMap<i64, Vec<String>>),
    Lost(String),
}

impl Daemon {
    fn connection(self) -> Result<Connection, Fail> {
        match self {
            Daemon::Ready(c, _) => Ok(c),
            Daemon::Lost(why) => Err((Exit::Io, why)),
            Daemon::Listing(_) => Err((Exit::Io, "History never arrived".into())),
        }
    }
}

impl picker::History for Daemon {
    fn clips(&mut self) -> Result<Vec<Clip>, String> {
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
        let types = clips.iter().map(|c| (c.id, c.types.clone())).collect();
        *self = Daemon::Ready(connection, types);
        Ok(clips
            .into_iter()
            .map(|c| Clip {
                id: c.id,
                kind: c.kind,
                at_ms: c
                    .at
                    .parse::<jiff::Timestamp>()
                    .ok()
                    .map(|t| t.as_millisecond()),
                preview: c.preview,
                app: c.app,
                pinned: c.pinned,
                concealed: c.sensitivity == hoarder_ipc::Sensitivity::Concealed,
            })
            .collect())
    }

    fn pin(&mut self, id: i64, pinned: bool) -> bool {
        let Daemon::Ready(connection, _) = self else {
            return false;
        };
        matches!(connection.pin(id, pinned), Ok(Ok(_)))
    }

    fn delete(&mut self, id: i64) -> bool {
        let Daemon::Ready(connection, _) = self else {
            return false;
        };
        matches!(connection.delete(id), Ok(Ok(())))
    }

    fn reveal(&mut self, id: i64) -> Option<String> {
        let Daemon::Ready(connection, _) = self else {
            return None;
        };
        let (_, pipe) = connection.get(id, None).ok()?.ok()?;
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        pipe.take(MAX_TEXT).read_to_end(&mut bytes).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn detail(&mut self, clip: &Clip) -> Detail {
        let Daemon::Ready(connection, types) = self else {
            return Detail::Nothing;
        };
        let types = types.get(&clip.id).map_or(&[][..], Vec::as_slice);
        let find = |want: &dyn Fn(&str) -> bool| types.iter().find(|m| want(m)).cloned();
        let mut read = |mime: String, max: u64| -> Option<Vec<u8>> {
            let (got, pipe) = connection.get(clip.id, Some(mime)).ok()?.ok()?;
            // Too big to decode, and never worth streaming whole.
            if got.size > MAX_IMAGE {
                return None;
            }
            let mut bytes = Vec::new();
            pipe.take(max).read_to_end(&mut bytes).ok()?;
            Some(bytes)
        };
        let detail = match clip.kind {
            Kind::Image => find(&|m| m == "image/png")
                .or_else(|| find(&|m| m.starts_with("image/")))
                .and_then(|m| read(m, MAX_IMAGE))
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
                .map(|image| {
                    let rgba = image.to_rgba8();
                    Detail::Image {
                        width: rgba.width(),
                        height: rgba.height(),
                        rgba: rgba.into_raw(),
                    }
                }),
            Kind::Files => find(&|m| m == "text/uri-list")
                .and_then(|m| read(m, MAX_TEXT))
                .map(|bytes| Detail::Files(file_paths(&String::from_utf8_lossy(&bytes)))),
            Kind::Text => find(&|m| m.starts_with("text/plain") || m == "UTF8_STRING")
                .or_else(|| find(&|m| m.starts_with("text/")))
                .and_then(|m| read(m, MAX_TEXT))
                .map(|bytes| Detail::Text(String::from_utf8_lossy(&bytes).into_owned())),
            Kind::Other => None,
        };
        detail.unwrap_or(Detail::Nothing)
    }

    fn search(&mut self, query: &str) -> Vec<i64> {
        let Daemon::Ready(connection, _) = self else {
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

/// The paths (or URIs, if not local files) of a `text/uri-list`.
fn file_paths(list: &str) -> Vec<String> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|uri| match uri.strip_prefix("file://") {
            // `file:///path` or `file://host/path`.
            Some(rest) => percent_decode(&rest[rest.find('/').unwrap_or(0)..]),
            None => uri.to_owned(),
        })
        .collect()
}

/// Undo `%XX` escapes; a broken escape stays as it is.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_list_reads_as_paths() {
        let list = "# copied\r\nfile:///tmp/report.pdf\r\nfile://host/home/me/notes%20today.md\r\nhttps://example.com/x\r\n";

        assert_eq!(
            file_paths(list),
            [
                "/tmp/report.pdf",
                "/home/me/notes today.md",
                "https://example.com/x"
            ]
        );
        assert_eq!(percent_decode("100%zz%41"), "100%zzA");
    }
}
