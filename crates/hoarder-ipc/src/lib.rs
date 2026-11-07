//! Request, response and event types for the Hoarder JSON-lines IPC API, plus a framing codec.
//!
//! The Daemon listens on `$XDG_RUNTIME_DIR/hoarder/hoarder.sock` ([`socket_path`]).
//! Every message is one JSON object on one line (ADR-0004, PLAN.md §11):
//!
//! ```text
//! → {"v":1,"id":7,"op":"status"}
//! ← {"v":1,"id":7,"ok":{…}}
//! ← {"v":1,"id":8,"err":{"code":"unknown_op","message":"…"}}
//! ```
//!
//! Unknown fields are ignored, so additive changes keep `v: 1`; breaking
//! changes bump [`PROTOCOL`] and this crate's major version.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::fd::{AsFd as _, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use serde_json::Value;

/// The protocol version this crate speaks.
pub const PROTOCOL: u32 = 1;

/// Every protocol version this crate understands, oldest first.
pub const SUPPORTED: &[u32] = &[1];

/// Where the Daemon listens: `$XDG_RUNTIME_DIR/hoarder/hoarder.sock`, or
/// `None` without a runtime directory.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("hoarder").join("hoarder.sock"))
}

/// One request from a Front end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Protocol version the request is written in.
    pub v: u32,
    /// Echoed in the response, so a Front end can match them up.
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// What a request asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Version negotiation; answered with [`Hello`].
    Hello,
    /// The Daemon's state; answered with [`Status`].
    Status,
    /// Empty a Selection (the clipboard unless `selection` says) and keep
    /// it empty: no Takeover restores it. Answered once the compositor has
    /// applied it.
    Clear {
        #[serde(default, skip_serializing_if = "Selection::is_clipboard")]
        selection: Selection,
    },
    /// Make the Daemon serve one payload as the Selection under each of
    /// `types`, in this order: what `hoarder copy` and Captures send. The
    /// payload is the whole of a regular file (a memfd) passed with the
    /// request (`SCM_RIGHTS`). Recorded in History unless `sensitive`.
    /// Answered once the compositor has applied it.
    Offer {
        #[serde(default, skip_serializing_if = "Selection::is_clipboard")]
        selection: Selection,
        types: Vec<String>,
        /// A password: offered with `x-kde-passwordManagerHint: secret`,
        /// never recorded nor restored.
        #[serde(default)]
        sensitive: bool,
        /// Serve one paste, then give the Selection up.
        #[serde(default)]
        once: bool,
    },
    /// Stream `mime` of the current Selection, straight from its source,
    /// into the pipe passed with the request (`SCM_RIGHTS`). Answered once
    /// the source has been asked; the pipe closes at the data's end.
    Paste {
        #[serde(default, skip_serializing_if = "Selection::is_clipboard")]
        selection: Selection,
        mime: String,
    },
    /// Receive [`Event`]s on this connection from now on, starting with a
    /// `selection` event for each Selection as it is now.
    Subscribe,
    /// Stream a Clip's thumbnail (a PNG at most 256 px on a side) into the
    /// pipe passed with the request; answered with [`Got`].
    Thumb { clip: i64 },
    /// History, most recently used first; answered with [`ClipSummary`]s.
    List {
        /// At most this many Clips (all of them if absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Stream one representation of a Clip into the pipe passed with the
    /// request (`SCM_RIGHTS`); answered with [`Got`] before the data
    /// arrives, and the pipe closes at its end.
    Get {
        clip: i64,
        /// The MIME type to send; the Clip's first type if absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
    },
    /// Make a Clip the clipboard Selection again, and move it to the top.
    /// Answered with its [`ClipSummary`] once the compositor has applied it.
    Restore { clip: i64 },
    /// Remove a Clip from History.
    Delete { clip: i64 },
    /// Exempt a Clip from expiry and eviction; answered with its
    /// [`ClipSummary`].
    Pin { clip: i64 },
    /// Make a pinned Clip ordinary again; answered with its [`ClipSummary`].
    Unpin { clip: i64 },
    /// Empty History, keeping pinned Clips unless `pinned`; answered with
    /// [`Cleared`]. (`clear` empties the clipboard.)
    ClearHistory {
        #[serde(default)]
        pinned: bool,
    },
    /// Stop recording into History, for `secs` seconds or until `resume`.
    /// Persistence and Takeover go on. Answered with [`Recording`].
    Pause {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secs: Option<u64>,
    },
    /// Record again after a Pause; answered with [`Recording`].
    Resume,
}

/// The `ok` of [`Op::ClearHistory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cleared {
    /// How many Clips went.
    pub deleted: u64,
}

/// Whether the Daemon records into History; the `ok` of [`Op::Pause`] and
/// [`Op::Resume`], and part of [`Status`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub state: State,
    /// When a timed Pause ends (RFC 3339); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// The Daemon's recording state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Recording every Normal copy.
    #[default]
    Active,
    /// Paused: nothing is recorded.
    Paused,
    #[serde(other)]
    Unknown,
}

/// Which Selection a request is about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// The regular clipboard (Ctrl+C / Ctrl+V).
    #[default]
    Clipboard,
    /// The primary selection (select / middle-click).
    Primary,
}

impl Selection {
    fn is_clipboard(&self) -> bool {
        *self == Selection::Clipboard
    }
}

/// Something that happened, pushed to subscribed connections
/// ([`Op::Subscribe`]) as `{"v":1,"event":…}` lines between responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A Selection changed hands: `types` is what it offers now, in the
    /// source's order, or `None` once it is empty.
    Selection {
        selection: Selection,
        types: Option<Vec<String>>,
    },
    /// An event newer than this crate.
    #[serde(other)]
    Unknown,
}

/// One event line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLine {
    pub v: u32,
    #[serde(flatten)]
    pub event: Event,
}

/// One Clip, as `list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipSummary {
    pub id: i64,
    /// `text`, `image`, `files` or `other`.
    pub kind: String,
    /// A line of text, an image's `W×H`, or the file names.
    pub preview: String,
    /// The app focused when it was copied, if known.
    #[serde(default)]
    pub app: Option<String>,
    /// When it was last copied or restored (RFC 3339).
    pub at: String,
    /// When it was first copied (RFC 3339).
    pub created: String,
    pub pinned: bool,
    /// Total bytes of all its representations.
    pub size: u64,
    /// Its MIME types, in the source's order.
    pub types: Vec<String>,
}

/// The `ok` of [`Op::Get`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Got {
    pub clip: ClipSummary,
    /// The type being streamed.
    pub mime: String,
    /// How many bytes will arrive.
    pub size: u64,
}

/// The Daemon's answer to one [`Request`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    /// The request's id; absent if the request was too broken to have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// Success (`ok`, op-specific) or failure (`err`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(Value),
    Err(Failure),
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Machine-readable failure kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Not JSON, or missing `v`, `id` or `op`.
    BadRequest,
    /// An `op` this Daemon does not know.
    UnknownOp,
    /// A `v` this Daemon does not speak; see [`SUPPORTED`].
    UnsupportedVersion,
    /// The Daemon understood but could not do it.
    Failed,
    /// No such Clip (or no such type in it).
    NotFound,
    /// Anything newer than this crate.
    #[serde(other)]
    Unknown,
}

/// The `ok` of [`Op::Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The newest protocol version the Daemon speaks.
    pub protocol: u32,
    /// Every version the Daemon speaks.
    pub versions: Vec<u32>,
    /// The Daemon's own version (`hoarder --version`).
    pub daemon: String,
    pub pid: u32,
}

/// The `ok` of [`Op::Status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub daemon: String,
    pub pid: u32,
    /// The data-control protocol in use: `ext` or `wlr`.
    pub data_control: String,
    /// Whether Takeover is on (`clipboard.persist`).
    pub persist: bool,
    /// Whether copies are recorded (Pause).
    #[serde(flatten)]
    pub recording: Recording,
    pub clipboard: SelectionStatus,
    /// `None` while the primary selection is ignored (`clipboard.primary`).
    pub primary: Option<SelectionStatus>,
}

/// What the Daemon knows about one Selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionStatus {
    pub owner: Owner,
    /// The MIME types offered right now, in the source's order.
    pub types: Vec<String>,
    /// The types read and held for Takeover, in the source's order.
    pub kept: Vec<String>,
    /// Whether the Daemon is still reading the current Offer.
    pub reading: bool,
    /// How private the current Selection is; `normal` until a read shows
    /// otherwise.
    #[serde(default)]
    pub sensitivity: Sensitivity,
}

/// How private a Selection change is (ADR-0006).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Stored in History.
    #[default]
    Normal,
    /// Looks private: held in locked memory only, masked, short-lived.
    Concealed,
    /// Marked by a password manager: never read, stored or restored.
    Secret,
}

/// Who serves a Selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    /// Nothing is copied.
    None,
    /// The app that set it.
    App,
    /// The Daemon, after a Takeover.
    Hoarder,
    #[serde(other)]
    Unknown,
}

/// Frame `message` as one line.
pub fn encode<T: Serialize>(message: &T) -> String {
    let mut line = serde_json::to_string(message).expect("IPC types always serialise");
    line.push('\n');
    line
}

/// Parse one request line. A line that is not a valid request yields the
/// error [`Response`] to send back instead.
pub fn parse_request(line: &str) -> Result<Request, Response> {
    let value: Value = serde_json::from_str(line).map_err(|e| Response {
        v: PROTOCOL,
        id: None,
        outcome: failure(ErrorCode::BadRequest, format!("not JSON: {e}")),
    })?;
    let id = value.get("id").and_then(Value::as_u64);
    let fail = |code, message: String| Response {
        v: PROTOCOL,
        id,
        outcome: failure(code, message),
    };
    let Some(v) = value.get("v").and_then(Value::as_u64) else {
        return Err(fail(ErrorCode::BadRequest, "missing \"v\"".into()));
    };
    if !SUPPORTED.iter().any(|s| u64::from(*s) == v) {
        return Err(fail(
            ErrorCode::UnsupportedVersion,
            format!("protocol version {v} is not supported (speaks {SUPPORTED:?})"),
        ));
    }
    if id.is_none() {
        return Err(fail(ErrorCode::BadRequest, "missing \"id\"".into()));
    }
    let Some(op) = value.get("op").and_then(Value::as_str).map(str::to_owned) else {
        return Err(fail(ErrorCode::BadRequest, "missing \"op\"".into()));
    };
    serde_json::from_value(value).map_err(|e| {
        if e.to_string().contains("unknown variant") {
            fail(ErrorCode::UnknownOp, format!("unknown op {op:?}"))
        } else {
            fail(ErrorCode::BadRequest, e.to_string())
        }
    })
}

fn failure(code: ErrorCode, message: String) -> Outcome {
    Outcome::Err(Failure { code, message })
}

/// One line from the Daemon.
enum Line {
    Response(Response),
    Event(Event),
}

/// A blocking connection to the Daemon, for one-shot Front ends like the CLI.
#[derive(Debug)]
pub struct Connection {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
    /// Events that arrived while waiting for a response.
    events: std::collections::VecDeque<Event>,
}

impl Connection {
    /// Connect to the Daemon at [`socket_path`].
    pub fn connect() -> std::io::Result<Self> {
        let path = socket_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set")
        })?;
        Self::connect_to(&path)
    }

    pub fn connect_to(path: &Path) -> std::io::Result<Self> {
        let writer = UnixStream::connect(path)?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Connection {
            writer,
            reader,
            next_id: 1,
            events: Default::default(),
        })
    }

    /// Send `op` and wait for its response.
    pub fn call(&mut self, op: Op) -> std::io::Result<Result<Value, Failure>> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            v: PROTOCOL,
            id,
            op,
        };
        self.writer.write_all(encode(&request).as_bytes())?;
        self.response(id)
    }

    /// Send `op` with `fd` attached (`SCM_RIGHTS`) and wait for its response.
    pub fn call_with_fd(
        &mut self,
        op: Op,
        fd: BorrowedFd<'_>,
    ) -> std::io::Result<Result<Value, Failure>> {
        use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
        let id = self.next_id;
        self.next_id += 1;
        let line = encode(&Request {
            v: PROTOCOL,
            id,
            op,
        });
        let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut space);
        let fds = [fd];
        ancillary.push(SendAncillaryMessage::ScmRights(&fds));
        let sent = sendmsg(
            &self.writer,
            &[std::io::IoSlice::new(line.as_bytes())],
            &mut ancillary,
            SendFlags::NOSIGNAL,
        )?;
        // The fd went with the first byte; the rest is plain stream.
        if sent < line.len() {
            self.writer.write_all(&line.as_bytes()[sent..])?;
        }
        self.response(id)
    }

    /// [`Op::Get`]: the response and a reader for the data, which ends
    /// after [`Got::size`] bytes.
    pub fn get(
        &mut self,
        clip: i64,
        mime: Option<String>,
    ) -> std::io::Result<Result<(Got, std::fs::File), Failure>> {
        self.streamed(Op::Get { clip, mime })
    }

    /// [`Op::Thumb`]: the response and a reader for the PNG.
    pub fn thumb(&mut self, clip: i64) -> std::io::Result<Result<(Got, std::fs::File), Failure>> {
        self.streamed(Op::Thumb { clip })
    }

    /// Send `op` with a pipe: its [`Got`] and the pipe's read end.
    fn streamed(&mut self, op: Op) -> std::io::Result<Result<(Got, std::fs::File), Failure>> {
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
        let response = self.call_with_fd(op, write.as_fd());
        drop(write);
        Ok(match response? {
            Ok(v) => Ok((
                serde_json::from_value(v)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
                std::fs::File::from(read),
            )),
            Err(f) => Err(f),
        })
    }

    /// [`Op::Offer`]: `data` goes over in a memfd.
    pub fn offer(
        &mut self,
        selection: Selection,
        types: Vec<String>,
        sensitive: bool,
        once: bool,
        data: &[u8],
    ) -> std::io::Result<Result<(), Failure>> {
        let memfd = rustix::fs::memfd_create("hoarder-offer", rustix::fs::MemfdFlags::CLOEXEC)?;
        let mut file = std::fs::File::from(memfd);
        file.write_all(data)?;
        let op = Op::Offer {
            selection,
            types,
            sensitive,
            once,
        };
        Ok(self.call_with_fd(op, file.as_fd())?.map(drop))
    }

    /// [`Op::Paste`]: a reader for the data, which ends with it.
    pub fn paste(
        &mut self,
        selection: Selection,
        mime: String,
    ) -> std::io::Result<Result<std::fs::File, Failure>> {
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
        let response = self.call_with_fd(Op::Paste { selection, mime }, write.as_fd());
        drop(write);
        Ok(response?.map(|_| std::fs::File::from(read)))
    }

    /// [`Op::Subscribe`]: events then arrive through [`Connection::next_event`].
    pub fn subscribe(&mut self) -> std::io::Result<Result<(), Failure>> {
        Ok(self.call(Op::Subscribe)?.map(drop))
    }

    /// The next event on a subscribed connection, waiting for it.
    pub fn next_event(&mut self) -> std::io::Result<Event> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        loop {
            if let Line::Event(event) = self.next_line()? {
                return Ok(event);
            }
        }
    }

    fn next_line(&mut self) -> std::io::Result<Line> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let invalid = |e| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
        let value: Value = serde_json::from_str(&line).map_err(invalid)?;
        Ok(if value.get("event").is_some() {
            Line::Event(
                serde_json::from_value::<EventLine>(value)
                    .map_err(invalid)?
                    .event,
            )
        } else {
            Line::Response(serde_json::from_value(value).map_err(invalid)?)
        })
    }

    /// [`Op::List`], decoded.
    pub fn list(
        &mut self,
        limit: Option<u32>,
    ) -> std::io::Result<Result<Vec<ClipSummary>, Failure>> {
        self.typed(Op::List { limit })
    }

    /// [`Op::Restore`], decoded.
    pub fn restore(&mut self, clip: i64) -> std::io::Result<Result<ClipSummary, Failure>> {
        self.typed(Op::Restore { clip })
    }

    /// [`Op::Pin`] or [`Op::Unpin`], decoded.
    pub fn pin(
        &mut self,
        clip: i64,
        pinned: bool,
    ) -> std::io::Result<Result<ClipSummary, Failure>> {
        self.typed(if pinned {
            Op::Pin { clip }
        } else {
            Op::Unpin { clip }
        })
    }

    /// [`Op::ClearHistory`], decoded.
    pub fn clear_history(&mut self, pinned: bool) -> std::io::Result<Result<Cleared, Failure>> {
        self.typed(Op::ClearHistory { pinned })
    }

    /// [`Op::Pause`], decoded.
    pub fn pause(&mut self, secs: Option<u64>) -> std::io::Result<Result<Recording, Failure>> {
        self.typed(Op::Pause { secs })
    }

    /// [`Op::Resume`], decoded.
    pub fn resume(&mut self) -> std::io::Result<Result<Recording, Failure>> {
        self.typed(Op::Resume)
    }

    /// Wait for the response to request `id`, keeping events that arrive
    /// meanwhile for [`Connection::next_event`].
    fn response(&mut self, id: u64) -> std::io::Result<Result<Value, Failure>> {
        loop {
            let response = match self.next_line()? {
                Line::Event(event) => {
                    self.events.push_back(event);
                    continue;
                }
                Line::Response(response) => response,
            };
            // Skip answers to anything else.
            if response.id == Some(id) {
                return Ok(match response.outcome {
                    Outcome::Ok(v) => Ok(v),
                    Outcome::Err(f) => Err(f),
                });
            }
        }
    }

    /// [`Op::Hello`], decoded.
    pub fn hello(&mut self) -> std::io::Result<Result<Hello, Failure>> {
        self.typed(Op::Hello)
    }

    /// [`Op::Status`], decoded.
    pub fn status(&mut self) -> std::io::Result<Result<Status, Failure>> {
        self.typed(Op::Status)
    }

    fn typed<T: for<'de> Deserialize<'de>>(
        &mut self,
        op: Op,
    ) -> std::io::Result<Result<T, Failure>> {
        Ok(match self.call(op)? {
            Ok(v) => Ok(serde_json::from_value(v)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?),
            Err(f) => Err(f),
        })
    }
}
