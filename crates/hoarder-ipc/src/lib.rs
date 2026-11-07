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
    /// Empty the clipboard and keep it empty: no Takeover restores it.
    /// Answered once the compositor has applied it.
    Clear,
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

/// A blocking connection to the Daemon, for one-shot Front ends like the CLI.
#[derive(Debug)]
pub struct Connection {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
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
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            let response: Response = serde_json::from_str(&line)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            // Skip anything else on the stream (events of a subscription).
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
