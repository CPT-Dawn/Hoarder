//! `hoarder copy` and `hoarder paste`, flag for flag compatible with
//! wl-copy and wl-paste (PLAN.md §5.2), and the serving child that keeps an
//! Offer alive after the command that made it has exited.

use std::io::{BufRead as _, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::Arc;

use clap::{Args, Parser};
use hoarder_wayland::clipboard::{Clipboard, ClipboardError, Representation, Selection, Serve};

use crate::Exit;

/// The KDE hint password managers set; its value is `secret`.
pub(crate) const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// The aliases X11-era apps look for, offered with every text copy.
const TEXT_ALIASES: [&str; 5] = [
    "text/plain",
    "text/plain;charset=utf-8",
    "TEXT",
    "STRING",
    "UTF8_STRING",
];

/// The serving child's handshake line, printed once its Offer is live.
const READY: &str = "ready";

/// `hoarder copy`: the flags of `wl-copy`.
#[derive(Debug, Args)]
pub struct CopyArgs {
    /// Use the primary selection.
    #[arg(short, long)]
    primary: bool,
    /// Offer the content as this MIME type instead of guessing.
    #[arg(short = 't', long = "type", value_name = "MIME")]
    mime_type: Option<String>,
    /// Drop one trailing newline.
    #[arg(short = 'n', long)]
    trim_newline: bool,
    /// Serve one paste, then give the selection up.
    #[arg(short = 'o', long)]
    paste_once: bool,
    /// Serve from this process instead of a background child.
    #[arg(short, long)]
    foreground: bool,
    /// Clear the selection instead of copying.
    #[arg(short, long)]
    clear: bool,
    /// Mark the content as a password (x-kde-passwordManagerHint).
    #[arg(long)]
    sensitive: bool,
    /// Use the seat with this name.
    #[arg(short, long, value_name = "NAME")]
    seat: Option<String>,
    /// Text to copy, joined with spaces. Without it, stdin is copied.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    text: Vec<String>,
}

/// `hoarder paste`: the flags of `wl-paste`.
#[derive(Debug, Args)]
pub struct PasteArgs {
    /// Do not append a newline to text.
    #[arg(short = 'n', long)]
    no_newline: bool,
    /// List the offered MIME types instead of pasting.
    #[arg(short, long)]
    list_types: bool,
    /// Use the primary selection.
    #[arg(short, long)]
    primary: bool,
    /// Run COMMAND with the content on stdin every time the selection changes.
    #[arg(short, long, value_name = "COMMAND", num_args = 1.., allow_hyphen_values = true)]
    watch: Option<Vec<String>>,
    /// Paste this MIME type: exact, `text` for any text, or a family like `image`.
    #[arg(short = 't', long = "type", value_name = "MIME")]
    mime_type: Option<String>,
    /// Use the seat with this name.
    #[arg(short, long, value_name = "NAME")]
    seat: Option<String>,
}

/// `wl-copy`, as the Compat shim: `hoarder copy` under wl-clipboard's name.
#[derive(Debug, Parser)]
#[command(
    name = "wl-copy",
    version,
    about = "Copy stdin or text to the Wayland clipboard"
)]
struct WlCopy {
    #[command(flatten)]
    args: CopyArgs,
}

/// `wl-paste`, as the Compat shim: `hoarder paste` under wl-clipboard's name.
#[derive(Debug, Parser)]
#[command(
    name = "wl-paste",
    version,
    about = "Paste the Wayland clipboard to stdout"
)]
struct WlPaste {
    #[command(flatten)]
    args: PasteArgs,
}

/// If this binary was started as `wl-copy` or `wl-paste` (argv[0], as the
/// `hoarder-wl-compat` symlinks do), run as that tool: its name and outcome.
pub fn run_as_shim() -> Option<(&'static str, Result<(), (Exit, String)>)> {
    let argv0 = std::env::args_os().next()?;
    let name = std::path::Path::new(&argv0).file_name()?.to_str()?;
    match name {
        "wl-copy" => Some(("wl-copy", copy(WlCopy::parse().args))),
        "wl-paste" => Some(("wl-paste", paste(WlPaste::parse().args))),
        _ => None,
    }
}

/// The hidden serving child: stdin holds the bytes, offered as each `--type`.
#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long)]
    primary: bool,
    #[arg(long = "type", value_name = "MIME", required = true)]
    mime_types: Vec<String>,
    #[arg(long)]
    sensitive: bool,
    #[arg(long)]
    paste_once: bool,
    #[arg(long)]
    seat: Option<String>,
}

fn which(primary: bool) -> Selection {
    if primary {
        Selection::Primary
    } else {
        Selection::Clipboard
    }
}

fn clipboard_error(e: ClipboardError) -> (Exit, String) {
    let code = match e {
        ClipboardError::NoWayland(_) | ClipboardError::Unsupported(_) => Exit::Unsupported,
        ClipboardError::Failed(_) | ClipboardError::Io(_) => Exit::Io,
    };
    (code, e.to_string())
}

/// Whether wl-clipboard treats `mime` as text (and so adds aliases or a newline).
fn is_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(mime, "TEXT" | "STRING" | "UTF8_STRING")
        || mime.contains("json")
        || ["script", "xml", "yaml", "csv", "ini"]
            .iter()
            .any(|s| mime.ends_with(s))
}

/// Guess a MIME type from content, the way `xdg-mime` sees common cases.
fn infer_mime_type(data: &[u8]) -> &'static str {
    const MAGIC: [(&[u8], &str); 6] = [
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
        (b"qoif", "image/qoi"),
    ];
    if let Some((_, mime)) = MAGIC.iter().find(|(m, _)| data.starts_with(m)) {
        return mime;
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return "image/webp";
    }
    if !data.contains(&0) && std::str::from_utf8(data).is_ok() {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

/// The MIME types a copy offers: the chosen one, then text aliases for text.
fn offered_types(mime: &str) -> Vec<String> {
    let mut types = vec![mime.to_owned()];
    if is_text(mime) {
        for alias in TEXT_ALIASES {
            if alias != mime {
                types.push(alias.to_owned());
            }
        }
    }
    types
}

pub fn copy(args: CopyArgs) -> Result<(), (Exit, String)> {
    let which = which(args.primary);
    if args.clear {
        // Through the Daemon, which would otherwise restore what was cleared.
        if args.seat.is_none() {
            return crate::daemon::clear(which);
        }
        let mut clipboard = Clipboard::connect(args.seat.as_deref()).map_err(clipboard_error)?;
        return clipboard.clear(which).map_err(clipboard_error);
    }
    let mut data = if args.text.is_empty() {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| (Exit::Io, format!("cannot read stdin: {e}")))?;
        buf
    } else {
        args.text.join(" ").into_bytes()
    };
    if args.trim_newline && data.last() == Some(&b'\n') {
        data.pop();
    }
    let mime = args
        .mime_type
        .clone()
        .unwrap_or_else(|| infer_mime_type(&data).to_owned());
    let offer = Offer {
        which,
        mime_types: offered_types(&mime),
        sensitive: args.sensitive,
        serve: if args.paste_once {
            Serve::Once
        } else {
            Serve::UntilReplaced
        },
        seat: args.seat,
    };
    if args.foreground {
        offer.serve_here(data.into(), || {})
    } else {
        offer.serve(&data)
    }
}

/// An Offer of one payload under several MIME types, ready to be served.
#[derive(Debug, Clone)]
pub struct Offer {
    pub which: Selection,
    pub mime_types: Vec<String>,
    pub sensitive: bool,
    pub serve: Serve,
    pub seat: Option<String>,
}

impl Offer {
    fn representations(&self, data: Arc<[u8]>) -> Vec<Representation> {
        let mut reprs: Vec<Representation> = self
            .mime_types
            .iter()
            .map(|m| Representation {
                mime_type: m.clone(),
                data: Arc::clone(&data),
            })
            .collect();
        if self.sensitive {
            reprs.push(Representation {
                mime_type: PASSWORD_HINT.into(),
                data: Arc::from(&b"secret"[..]),
            });
        }
        reprs
    }

    /// Serve from this process until the Offer is replaced (or pasted, once).
    fn serve_here(&self, data: Arc<[u8]>, ready: impl FnOnce()) -> Result<(), (Exit, String)> {
        let clipboard = Clipboard::connect(self.seat.as_deref()).map_err(clipboard_error)?;
        clipboard
            .serve(self.which, self.representations(data), self.serve, ready)
            .map_err(clipboard_error)
    }

    /// Serve `data` after this process exits: by the Daemon when it runs,
    /// else by a background child.
    pub fn serve(&self, data: &[u8]) -> Result<(), (Exit, String)> {
        match self.hand_to_daemon(data) {
            Some(result) => result,
            None => self.serve_in_background(data),
        }
    }

    /// Hand `data` to the Daemon (`offer`); `None` if no Daemon takes it,
    /// so the caller serves it some other way.
    fn hand_to_daemon(&self, data: &[u8]) -> Option<Result<(), (Exit, String)>> {
        // The Daemon watches the default seat only.
        if self.seat.is_some() {
            return None;
        }
        let mut daemon = hoarder_ipc::Connection::connect().ok()?;
        // The Daemon adds the password hint itself.
        let types = self
            .mime_types
            .iter()
            .filter(|m| *m != PASSWORD_HINT)
            .cloned()
            .collect();
        let once = self.serve == Serve::Once;
        let which = crate::daemon::to_ipc(self.which);
        match daemon.offer(which, types, self.sensitive, once, data) {
            Ok(Ok(())) => Some(Ok(())),
            // A Daemon too old for `offer`, or one that went away.
            Ok(Err(f)) if f.code == hoarder_ipc::ErrorCode::UnknownOp => None,
            Err(_) => None,
            Ok(Err(f)) => Some(Err((Exit::Io, f.message))),
        }
    }

    /// Hand `data` to a detached child of this binary that serves it, and
    /// return once the Offer is live, like wl-copy's fork. The child holds no
    /// terminal, stdio or working directory of ours.
    pub fn serve_in_background(&self, data: &[u8]) -> Result<(), (Exit, String)> {
        let exe = std::env::current_exe()
            .map_err(|e| (Exit::Io, format!("cannot find the hoarder binary: {e}")))?;
        let mut cmd = Command::new(exe);
        cmd.arg(SERVE_SUBCOMMAND);
        if self.which == Selection::Primary {
            cmd.arg("--primary");
        }
        for m in &self.mime_types {
            cmd.arg("--type").arg(m);
        }
        if self.sensitive {
            cmd.arg("--sensitive");
        }
        if self.serve == Serve::Once {
            cmd.arg("--paste-once");
        }
        if let Some(seat) = &self.seat {
            cmd.arg("--seat").arg(seat);
        }
        let spawn_err = |e: std::io::Error| (Exit::Io, format!("cannot start serving: {e}"));
        let mut child = std::os::unix::process::CommandExt::process_group(&mut cmd, 0)
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawn_err)?;
        let mut stdin = child.stdin.take().expect("piped");
        stdin.write_all(data).map_err(spawn_err)?;
        drop(stdin);
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("piped"))
            .read_line(&mut line)
            .map_err(spawn_err)?;
        if line.trim_end() == READY {
            return Ok(());
        }
        // The child died before its Offer went live: pass its complaint on.
        let mut why = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut why);
        }
        let status = child.wait().map_err(spawn_err)?;
        let code = match status.code() {
            Some(3) => Exit::Unsupported,
            _ => Exit::Io,
        };
        let why = why.trim().trim_start_matches("hoarder: ");
        Err((code, why.to_owned()))
    }
}

/// Put a Capture's PNG on the clipboard so it stays pasteable after
/// `hoarder shot` exits. The Daemon takes it and records it as an image
/// Clip; with no Daemon running a serving child holds it, and the user hears
/// once that History is off.
pub fn copy_capture(png: &[u8]) -> Result<(), (Exit, String)> {
    let offer = Offer {
        which: Selection::Clipboard,
        mime_types: vec!["image/png".into()],
        sensitive: false,
        serve: Serve::UntilReplaced,
        seat: None,
    };
    if let Some(result) = offer.hand_to_daemon(png) {
        return result;
    }
    eprintln!(
        "hoarder: History is off (no Daemon running); the Capture is on the clipboard \
         but will not be kept. Run `hoarder service enable` to keep Captures."
    );
    offer.serve_in_background(png)
}

/// The hidden subcommand the background child runs.
pub const SERVE_SUBCOMMAND: &str = "serve-offer";

pub fn serve_child(args: ServeArgs) -> Result<(), (Exit, String)> {
    let mut data = Vec::new();
    std::io::stdin()
        .read_to_end(&mut data)
        .map_err(|e| (Exit::Io, format!("cannot read the offer: {e}")))?;
    let offer = Offer {
        which: which(args.primary),
        mime_types: args.mime_types,
        sensitive: args.sensitive,
        serve: if args.paste_once {
            Serve::Once
        } else {
            Serve::UntilReplaced
        },
        seat: args.seat,
    };
    let mut live = false;
    let result = offer.serve_here(data.into(), || {
        live = true;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{READY}");
        let _ = out.flush();
    });
    match result {
        // Nobody is listening once the parent has gone: end quietly.
        Err(_) if live => Ok(()),
        other => other,
    }
}

/// Pick the offered type wl-paste would: an exact match, `text` for any text,
/// a bare family like `image`, or by default the best text type, else the first.
fn choose_type<'a>(offered: &'a [String], wanted: Option<&str>) -> Option<&'a str> {
    let offered = offered.iter().map(String::as_str);
    match wanted {
        Some(w) => offered.clone().find(|m| *m == w).or_else(|| match w {
            "text" => offered.clone().find(|m| is_text(m)),
            family if !family.contains('/') => offered.clone().find(|m| {
                m.strip_prefix(family)
                    .is_some_and(|rest| rest.starts_with('/'))
            }),
            _ => None,
        }),
        None => offered
            .clone()
            .find(|m| *m == "text/plain;charset=utf-8")
            .or_else(|| offered.clone().find(|m| is_text(m)))
            .or_else(|| offered.clone().find(|m| *m != PASSWORD_HINT)),
    }
}

pub fn paste(args: PasteArgs) -> Result<(), (Exit, String)> {
    let which = which(args.primary);
    if let Some(command) = &args.watch {
        return watch(which, &args, command);
    }
    let mut clipboard = Clipboard::connect(args.seat.as_deref()).map_err(clipboard_error)?;
    let Some(offered) = clipboard.offered(which).map(<[String]>::to_vec) else {
        return Err((Exit::Cancelled, "Nothing is copied".into()));
    };
    let mut stdout = std::io::stdout().lock();
    let io = |e: std::io::Error| (Exit::Io, format!("cannot write to stdout: {e}"));
    if args.list_types {
        for m in &offered {
            writeln!(stdout, "{m}").map_err(io)?;
        }
        return stdout.flush().map_err(io);
    }
    let mime = choose_type(&offered, args.mime_type.as_deref()).ok_or_else(|| {
        let wanted = args.mime_type.as_deref().unwrap_or("any");
        (
            Exit::Cancelled,
            format!(
                "Clipboard content is not available as requested type \"{wanted}\"\n\
                 Use \"hoarder paste --list-types\" to view available types."
            ),
        )
    })?;
    let data = clipboard.read(which, mime).map_err(clipboard_error)?;
    stdout.write_all(&data).map_err(io)?;
    if !args.no_newline && is_text(mime) {
        stdout.write_all(b"\n").map_err(io)?;
    }
    stdout.flush().map_err(io)
}

/// Where `paste --watch` hears of changes and reads them from.
enum Changes {
    /// The Daemon's event stream (`subscribe`); reads go through it too.
    Daemon(hoarder_ipc::Connection),
    /// A data-control connection of our own, when no Daemon runs.
    Own { clipboard: Clipboard, started: bool },
}

impl Changes {
    /// The Daemon's event stream if it runs (and the default seat is
    /// wanted), else a connection of our own.
    fn open(seat: Option<&str>) -> Result<Changes, (Exit, String)> {
        if seat.is_none()
            && let Ok(mut daemon) = hoarder_ipc::Connection::connect()
            && let Ok(Ok(())) = daemon.subscribe()
        {
            return Ok(Changes::Daemon(daemon));
        }
        let clipboard = Clipboard::connect(seat).map_err(clipboard_error)?;
        Ok(Changes::Own {
            clipboard,
            started: false,
        })
    }

    /// The types `which` offers: as it is now the first time, then after
    /// each change (`None` while it is empty).
    fn next(&mut self, which: Selection) -> Result<Option<Vec<String>>, (Exit, String)> {
        match self {
            Changes::Daemon(daemon) => loop {
                let event = daemon.next_event().map_err(lost_daemon)?;
                if let hoarder_ipc::Event::Selection { selection, types } = event
                    && selection == crate::daemon::to_ipc(which)
                {
                    return Ok(types);
                }
            },
            Changes::Own { clipboard, started } if !*started => {
                *started = true;
                Ok(clipboard.offered(which).map(<[String]>::to_vec))
            }
            Changes::Own { clipboard, .. } => clipboard.next_change(which).map_err(clipboard_error),
        }
    }

    fn read(&mut self, which: Selection, mime: &str) -> Result<Vec<u8>, (Exit, String)> {
        match self {
            Changes::Daemon(daemon) => {
                let which = crate::daemon::to_ipc(which);
                let mut data = Vec::new();
                // Gone again before we asked: nothing to hand on.
                if let Ok(mut pipe) = daemon.paste(which, mime.into()).map_err(lost_daemon)? {
                    pipe.read_to_end(&mut data).map_err(lost_daemon)?;
                }
                Ok(data)
            }
            Changes::Own { clipboard, .. } => clipboard.read(which, mime).map_err(clipboard_error),
        }
    }
}

fn lost_daemon(e: std::io::Error) -> (Exit, String) {
    (Exit::Io, format!("lost the Daemon: {e}"))
}

/// `paste --watch`: run `command` for the current selection and every change.
fn watch(which: Selection, args: &PasteArgs, command: &[String]) -> Result<(), (Exit, String)> {
    let mut changes = Changes::open(args.seat.as_deref())?;
    loop {
        let (state, data) = match changes.next(which)? {
            None => ("nil", Vec::new()),
            Some(types) => {
                let state = if types.iter().any(|m| m == PASSWORD_HINT) {
                    "sensitive"
                } else {
                    "data"
                };
                match choose_type(&types, args.mime_type.as_deref()) {
                    Some(mime) => (state, changes.read(which, mime)?),
                    None => (state, Vec::new()),
                }
            }
        };
        run_watcher(command, state, &data)?;
    }
}

fn run_watcher(command: &[String], state: &str, data: &[u8]) -> Result<(), (Exit, String)> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .env("CLIPBOARD_STATE", state)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| (Exit::Io, format!("cannot run {}: {e}", command[0])))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A watcher that ignores its input is fine.
        let _ = stdin.write_all(data);
    }
    let _ = child.wait();
    Ok(())
}
