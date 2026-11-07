//! The Daemon's end of the socket (PLAN.md §11): a private directory, a
//! same-user check on every connection, and JSON-lines requests answered on
//! the event loop.

use std::collections::HashMap;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};

use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};
use hoarder_ipc::{Failure, Hello, Op, Outcome, PROTOCOL, Response, SUPPORTED, Value};

use super::{Daemon, Fail};
use crate::Exit;

/// A request line longer than this is not a Front end talking.
const MAX_LINE: usize = 1024 * 1024;

/// Connected Front ends, by connection number.
#[derive(Debug, Default)]
pub(super) struct Clients {
    next: u64,
    /// The write half of each connection and its unfinished input line.
    open: HashMap<u64, (UnixStream, Vec<u8>)>,
}

/// Create `$XDG_RUNTIME_DIR/hoarder` (mode 0700) and listen on its socket.
/// Refuses if another Daemon already answers there.
pub(super) fn bind() -> Result<UnixListener, Fail> {
    let path = hoarder_ipc::socket_path().ok_or_else(|| {
        (
            Exit::Io,
            "XDG_RUNTIME_DIR is not set; nowhere to put the socket".into(),
        )
    })?;
    let dir = path.parent().expect("socket path has a directory");
    let io =
        |what: &str, e: std::io::Error| (Exit::Io, format!("cannot {what} {}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(|e| io("create", e))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| io("secure", e))?;
    if UnixStream::connect(&path).is_ok() {
        return Err((
            Exit::Io,
            format!("another Daemon is already running on {}", path.display()),
        ));
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io("clean up a stale socket in", e)),
    }
    let listener = UnixListener::bind(&path).map_err(|e| {
        (
            Exit::Io,
            format!("cannot listen on {}: {e}", path.display()),
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|e| io("configure the socket in", e))?;
    Ok(listener)
}

pub(super) fn listen(
    handle: &LoopHandle<'static, Daemon>,
    listener: UnixListener,
) -> Result<(), Fail> {
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, daemon: &mut Daemon| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => daemon.accept(stream),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        Err(e) => {
                            eprintln!("hoarder: cannot accept a connection: {e}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| (Exit::Io, format!("event loop: {}", e.error)))?;
    Ok(())
}

/// Whether the peer runs as the same user as the Daemon.
fn same_user(stream: &UnixStream) -> bool {
    rustix::net::sockopt::socket_peercred(stream)
        .is_ok_and(|cred| cred.uid == rustix::process::getuid())
}

impl Daemon {
    fn accept(&mut self, stream: UnixStream) {
        if !same_user(&stream) || stream.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(writer) = stream.try_clone() else {
            return;
        };
        let id = self.clients.next;
        self.clients.next += 1;
        let inserted = self.handle.insert_source(
            Generic::new(stream, Interest::READ, Mode::Level),
            move |_, stream, daemon: &mut Daemon| Ok(daemon.on_client(id, stream)),
        );
        if inserted.is_ok() {
            self.clients.open.insert(id, (writer, Vec::new()));
        }
    }

    fn on_client(&mut self, id: u64, stream: &UnixStream) -> PostAction {
        let mut chunk = [0u8; 4096];
        loop {
            match (&*stream).read(&mut chunk) {
                Ok(0) => {
                    self.clients.open.remove(&id);
                    return PostAction::Remove;
                }
                Ok(n) => {
                    let Some((_, buf)) = self.clients.open.get_mut(&id) else {
                        return PostAction::Remove;
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > MAX_LINE && !buf.contains(&b'\n') {
                        self.clients.open.remove(&id);
                        return PostAction::Remove;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.clients.open.remove(&id);
                    return PostAction::Remove;
                }
            }
        }
        while let Some(line) = self.take_line(id) {
            self.request(id, &line);
        }
        if self.clients.open.contains_key(&id) {
            PostAction::Continue
        } else {
            PostAction::Remove
        }
    }

    fn take_line(&mut self, id: u64) -> Option<String> {
        let (_, buf) = self.clients.open.get_mut(&id)?;
        let end = buf.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = buf.drain(..=end).collect();
        Some(String::from_utf8_lossy(&line).trim().to_owned())
    }

    fn request(&mut self, client: u64, line: &str) {
        if line.is_empty() {
            return;
        }
        let request = match hoarder_ipc::parse_request(line) {
            Ok(r) => r,
            Err(response) => return self.send_response(client, &response),
        };
        let id = request.id;
        match request.op {
            Op::Hello => {
                let hello = Hello {
                    protocol: PROTOCOL,
                    versions: SUPPORTED.to_vec(),
                    daemon: env!("CARGO_PKG_VERSION").into(),
                    pid: std::process::id(),
                };
                self.reply(client, id, Ok(to_value(&hello)));
            }
            Op::Status => {
                let status = self.status();
                self.reply(client, id, Ok(to_value(&status)));
            }
            Op::Clear => self.clear(client, id),
        }
    }

    pub(super) fn reply(&mut self, client: u64, id: u64, result: Result<Value, Failure>) {
        let outcome = match result {
            Ok(v) => Outcome::Ok(v),
            Err(f) => Outcome::Err(f),
        };
        let response = Response {
            v: PROTOCOL,
            id: Some(id),
            outcome,
        };
        self.send_response(client, &response);
    }

    fn send_response(&mut self, client: u64, response: &Response) {
        let line = hoarder_ipc::encode(response);
        let Some((writer, _)) = self.clients.open.get_mut(&client) else {
            return;
        };
        // Replies are small; a Front end that cannot take one is dropped.
        if writer.write_all(line.as_bytes()).is_err() {
            self.clients.open.remove(&client);
        }
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
