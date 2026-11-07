//! The Daemon's end of the socket (PLAN.md §11): a private directory, a
//! same-user check on every connection, and JSON-lines requests answered on
//! the event loop. Ops that take a file descriptor (`get`, `thumb`, `paste`,
//! `offer`) take the oldest one passed (`SCM_RIGHTS`) on that connection and
//! not yet used. Subscribed connections also get event lines.

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Write as _};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};

use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};
use hoarder_ipc::{
    ErrorCode, EventLine, Failure, Hello, Op, Outcome, PROTOCOL, Response, SUPPORTED, Value,
};
use rustix::io::Errno;
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};

use super::history::{Job, to_value};
use super::{Daemon, Fail};
use crate::Exit;

/// A request line longer than this is not a Front end talking.
const MAX_LINE: usize = 1024 * 1024;
/// Passed fds a connection may have waiting; more is not a Front end either.
const MAX_FDS: usize = 16;
/// Reply bytes a connection may leave unread.
const MAX_PENDING: usize = 64 * 1024 * 1024;

/// Connected Front ends, by connection number.
#[derive(Debug, Default)]
pub(super) struct Clients {
    next: u64,
    open: HashMap<u64, Client>,
}

/// One connected Front end.
#[derive(Debug)]
struct Client {
    /// The write half of the connection.
    writer: UnixStream,
    /// Its unfinished input line.
    buf: Vec<u8>,
    /// Fds it passed, oldest first, not yet taken by a request.
    fds: VecDeque<OwnedFd>,
    /// Reply bytes the socket had no room for yet.
    pending: Vec<u8>,
    /// Whether it gets events (`subscribe`).
    subscribed: bool,
    /// Requests it is still owed a response to.
    awaiting: usize,
    /// It has sent all it will (`echo … | socat`): answer, then hang up.
    eof: bool,
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
            self.clients.open.insert(
                id,
                Client {
                    writer,
                    buf: Vec::new(),
                    fds: VecDeque::new(),
                    pending: Vec::new(),
                    subscribed: false,
                    awaiting: 0,
                    eof: false,
                },
            );
        }
    }

    fn on_client(&mut self, id: u64, stream: &UnixStream) -> PostAction {
        let mut chunk = [0u8; 4096];
        loop {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(4))];
            let mut ancillary = RecvAncillaryBuffer::new(&mut space);
            let received = recvmsg(
                stream,
                &mut [std::io::IoSliceMut::new(&mut chunk)],
                &mut ancillary,
                RecvFlags::CMSG_CLOEXEC,
            );
            match received {
                Ok(msg) => {
                    let Some(client) = self.clients.open.get_mut(&id) else {
                        return PostAction::Remove;
                    };
                    for message in ancillary.drain() {
                        if let RecvAncillaryMessage::ScmRights(fds) = message {
                            client.fds.extend(fds);
                        }
                    }
                    let n = msg.bytes;
                    client.buf.extend_from_slice(&chunk[..n]);
                    let flooded = client.buf.len() > MAX_LINE && !client.buf.contains(&b'\n');
                    if flooded || client.fds.len() > MAX_FDS {
                        self.clients.open.remove(&id);
                        return PostAction::Remove;
                    }
                    if n == 0 {
                        // A last line may lack its newline.
                        client.eof = true;
                        if !client.buf.is_empty() {
                            client.buf.push(b'\n');
                        }
                        break;
                    }
                }
                Err(Errno::AGAIN) => break,
                Err(Errno::INTR) => {}
                Err(_) => {
                    self.clients.open.remove(&id);
                    return PostAction::Remove;
                }
            }
        }
        while let Some(line) = self.take_line(id) {
            self.request(id, &line);
        }
        match self.clients.open.get(&id) {
            Some(client) if !client.eof => PostAction::Continue,
            // Nothing more to read; the connection lives on for its answers.
            Some(_) => {
                self.hang_up_if_done(id);
                PostAction::Remove
            }
            None => PostAction::Remove,
        }
    }

    /// Drop a client that has stopped sending once it has had every answer
    /// it is owed. A subscriber stays until a write to it fails.
    fn hang_up_if_done(&mut self, id: u64) {
        let done = self.clients.open.get(&id).is_some_and(|c| {
            c.eof && c.buf.is_empty() && c.awaiting == 0 && c.pending.is_empty() && !c.subscribed
        });
        if done {
            self.clients.open.remove(&id);
        }
    }

    fn take_line(&mut self, id: u64) -> Option<String> {
        let buf = &mut self.clients.open.get_mut(&id)?.buf;
        let end = buf.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = buf.drain(..=end).collect();
        Some(String::from_utf8_lossy(&line).trim().to_owned())
    }

    fn request(&mut self, client: u64, line: &str) {
        if line.is_empty() {
            return;
        }
        if let Some(c) = self.clients.open.get_mut(&client) {
            c.awaiting += 1;
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
            Op::Reload => {
                let result = self
                    .reload()
                    .map(|()| Value::Null)
                    .map_err(|message| Failure {
                        code: ErrorCode::Failed,
                        message,
                    });
                self.reply(client, id, result);
            }
            Op::Clear { selection } => self.clear(client, id, super::from_ipc(selection)),
            Op::List { limit } => self.ask_history(client, id, Job::List { client, id, limit }),
            Op::Search { query, limit } => {
                let job = Job::Search {
                    client,
                    id,
                    query,
                    limit,
                };
                self.ask_history(client, id, job);
            }
            Op::Get { clip, mime } => {
                let Some(pipe) = self.take_fd(client, id, "get streams into a pipe") else {
                    return;
                };
                let job = Job::Get {
                    client,
                    id,
                    clip,
                    mime,
                    pipe,
                };
                self.ask_history(client, id, job);
            }
            Op::Thumb { clip } => {
                let Some(pipe) = self.take_fd(client, id, "thumb streams into a pipe") else {
                    return;
                };
                let job = Job::Thumb {
                    client,
                    id,
                    clip,
                    pipe,
                };
                self.ask_history(client, id, job);
            }
            Op::Restore { clip } => self.ask_history(client, id, Job::Restore { client, id, clip }),
            Op::Delete { clip } => self.ask_history(client, id, Job::Delete { client, id, clip }),
            Op::Offer {
                selection,
                types,
                sensitive,
                once,
            } => {
                let Some(file) = self.take_fd(client, id, "offer data comes in a memfd") else {
                    return;
                };
                if types.is_empty() {
                    return self.reply(client, id, Err(bad_request("offer needs types")));
                }
                match read_payload(file) {
                    Ok(data) => {
                        let offer = super::OfferRequest {
                            which: super::from_ipc(selection),
                            types,
                            sensitive,
                            once,
                            data,
                        };
                        self.offer(client, id, offer);
                    }
                    Err(failure) => self.reply(client, id, Err(failure)),
                }
            }
            Op::Paste { selection, mime } => {
                let Some(pipe) = self.take_fd(client, id, "paste streams into a pipe") else {
                    return;
                };
                let result = self
                    .paste(super::from_ipc(selection), &mime, pipe)
                    .map(|()| Value::Null)
                    .map_err(|message| Failure {
                        code: ErrorCode::NotFound,
                        message,
                    });
                self.reply(client, id, result);
            }
            Op::Subscribe => {
                if let Some(c) = self.clients.open.get_mut(&client) {
                    c.subscribed = true;
                }
                self.reply(client, id, Ok(Value::Null));
                for which in super::SELECTIONS {
                    let event = hoarder_ipc::Event::Selection {
                        selection: super::to_ipc(which),
                        types: self.current[which as usize]
                            .as_ref()
                            .map(|o| o.mime_types.clone()),
                    };
                    self.send_event(client, &event);
                }
                let state = hoarder_ipc::Event::State(self.recording());
                self.send_event(client, &state);
            }
            Op::Pin { clip } | Op::Unpin { clip } => {
                let pinned = matches!(request.op, Op::Pin { .. });
                self.ask_history(
                    client,
                    id,
                    Job::Pin {
                        client,
                        id,
                        clip,
                        pinned,
                    },
                );
            }
            Op::ClearHistory { pinned } => {
                self.ask_history(client, id, Job::Clear { client, id, pinned })
            }
            Op::Pause { secs } => {
                let recording = self.pause(secs);
                self.reply(client, id, Ok(to_value(&recording)));
            }
            Op::Resume => {
                let recording = self.resume();
                self.reply(client, id, Ok(to_value(&recording)));
            }
            // `parse_request` only yields ops this crate knows, and this
            // Daemon handles them all; a newer hoarder-ipc is caught here.
            _ => self.reply(
                client,
                id,
                Err(Failure {
                    code: ErrorCode::UnknownOp,
                    message: "this Daemon does not handle that op yet".into(),
                }),
            ),
        }
    }

    /// The oldest fd `client` passed and no request has taken yet; without
    /// one, answers `id` with `needs` and returns `None`.
    fn take_fd(&mut self, client: u64, id: u64, needs: &str) -> Option<OwnedFd> {
        let fd = self
            .clients
            .open
            .get_mut(&client)
            .and_then(|c| c.fds.pop_front());
        if fd.is_none() {
            let why = format!("{needs} passed with the request (SCM_RIGHTS)");
            self.reply(client, id, Err(bad_request(&why)));
        }
        fd
    }

    /// Tell every subscribed Front end about `event`.
    pub(super) fn broadcast(&mut self, event: &hoarder_ipc::Event) {
        let subscribed: Vec<u64> = self
            .clients
            .open
            .iter()
            .filter(|(_, c)| c.subscribed)
            .map(|(id, _)| *id)
            .collect();
        for client in subscribed {
            self.send_event(client, event);
        }
    }

    fn send_event(&mut self, client: u64, event: &hoarder_ipc::Event) {
        let line = hoarder_ipc::encode(&EventLine {
            v: PROTOCOL,
            event: event.clone(),
        });
        self.send_line(client, &line);
    }

    /// Hand a request to the History thread, which answers it.
    fn ask_history(&mut self, client: u64, id: u64, job: Job) {
        if let Err(failure) = self.history.send(job) {
            self.reply(client, id, Err(failure));
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
        if let Some(c) = self.clients.open.get_mut(&client) {
            c.awaiting = c.awaiting.saturating_sub(1);
        }
        self.send_line(client, &hoarder_ipc::encode(response));
        self.hang_up_if_done(client);
    }

    fn send_line(&mut self, client: u64, line: &str) {
        let Some(Client {
            writer, pending, ..
        }) = self.clients.open.get_mut(&client)
        else {
            return;
        };
        let writer_busy = !pending.is_empty();
        let unsent = if writer_busy {
            line.as_bytes()
        } else {
            match write_some(writer, line.as_bytes()) {
                Ok(n) => &line.as_bytes()[n..],
                Err(_) => {
                    self.clients.open.remove(&client);
                    return;
                }
            }
        };
        if unsent.is_empty() {
            return;
        }
        if unsent.len() + pending.len() > MAX_PENDING {
            // A Front end that reads nothing is not worth our memory.
            self.clients.open.remove(&client);
            return;
        }
        pending.extend_from_slice(unsent);
        if writer_busy {
            return;
        }
        // Finish the reply whenever the Front end makes room for it.
        let Ok(fd) = writer.try_clone() else {
            self.clients.open.remove(&client);
            return;
        };
        let inserted = self.handle.insert_source(
            Generic::new(fd, Interest::WRITE, Mode::Level),
            move |_, _, daemon: &mut Daemon| Ok(daemon.on_writable(client)),
        );
        if inserted.is_err() {
            self.clients.open.remove(&client);
        }
    }

    /// Send what is pending for `client`; stop watching once all is sent.
    fn on_writable(&mut self, client: u64) -> PostAction {
        let Some(c) = self.clients.open.get_mut(&client) else {
            return PostAction::Remove;
        };
        match write_some(&mut c.writer, &c.pending) {
            Ok(n) => {
                c.pending.drain(..n);
                if c.pending.is_empty() {
                    self.hang_up_if_done(client);
                    PostAction::Remove
                } else {
                    PostAction::Continue
                }
            }
            Err(_) => {
                self.clients.open.remove(&client);
                PostAction::Remove
            }
        }
    }
}

fn bad_request(message: &str) -> Failure {
    Failure {
        code: ErrorCode::BadRequest,
        message: message.into(),
    }
}

/// The whole of an `offer`'s payload file. Only a regular file (a memfd)
/// will do: a pipe could keep the loop waiting forever.
fn read_payload(fd: OwnedFd) -> Result<Vec<u8>, Failure> {
    use std::os::unix::fs::FileExt as _;
    let stat = rustix::fs::fstat(&fd).map_err(|e| bad_request(&e.to_string()))?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(bad_request(
            "offer data comes in a regular file (a memfd), not a pipe",
        ));
    }
    let file = std::fs::File::from(fd);
    let mut data = vec![0; stat.st_size as usize];
    file.read_exact_at(&mut data, 0).map_err(|e| Failure {
        code: ErrorCode::Failed,
        message: format!("cannot read the offer: {e}"),
    })?;
    Ok(data)
}

/// Write as much of `data` as the socket takes now.
fn write_some(writer: &mut UnixStream, data: &[u8]) -> std::io::Result<usize> {
    let mut sent = 0;
    while sent < data.len() {
        match writer.write(&data[sent..]) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => sent += n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(sent)
}
