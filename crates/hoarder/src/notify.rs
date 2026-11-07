//! The Capture notification (PLAN.md §4.1 step 7): a thumbnail plus Open,
//! Edit, Show in folder and Delete, over org.freedesktop.Notifications.
//!
//! Answering an action needs a process that outlives `hoarder shot`, so a
//! detached child of this binary sends the notification, tells the shot how it
//! went, and then waits for a click until the notification closes. When the
//! Daemon lands it can take this job over.

use std::collections::HashMap;
use std::io::{BufRead as _, Write as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use clap::Args;
use futures_util::StreamExt as _;
use zbus::zvariant::Variant;

use crate::Exit;
use crate::after::{Delivered, on_path, run_detached};

/// The hidden subcommand the notifying child runs.
pub const SUBCOMMAND: &str = "notify-capture";

/// Stop waiting for a click after this long, so no child lingers for ever
/// behind a server that never reports the notification closed.
const LONGEST_WAIT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Args)]
pub struct NotifyArgs {
    /// The saved file, if the Capture went to one.
    #[arg(long)]
    path: Option<PathBuf>,
    #[arg(long)]
    width: u32,
    #[arg(long)]
    height: u32,
    /// A badge such as `mixed scale` or `HDR`; may be repeated.
    #[arg(long = "badge")]
    badges: Vec<String>,
}

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.FileManager1",
    default_service = "org.freedesktop.FileManager1",
    default_path = "/org/freedesktop/FileManager1"
)]
trait FileManager1 {
    fn show_items(&self, uris: &[&str], startup_id: &str) -> zbus::Result<()>;
}

/// What the child reports on its stdout, one line.
const SHOWN: &str = "shown";
/// No bus or no notification server: not worth a warning.
const NO_SERVER: &str = "no-server";

/// Announce `capture`, returning once the notification is up (or has failed).
pub fn spawn(capture: &Delivered) {
    if let Err(why) = try_spawn(capture) {
        eprintln!("hoarder: cannot show the notification: {why}");
    }
}

fn try_spawn(capture: &Delivered) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg(SUBCOMMAND)
        .arg("--width")
        .arg(capture.width.to_string())
        .arg("--height")
        .arg(capture.height.to_string());
    for badge in capture.badges.labels() {
        cmd.arg("--badge").arg(badge);
    }
    if let Some(path) = &capture.path {
        cmd.arg("--path")
            .arg(std::path::absolute(path).map_err(|e| e.to_string())?);
    }
    let mut child = cmd
        .process_group(0)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("piped"))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    // The child lives on to answer clicks; reap it if it ends early.
    std::thread::spawn(move || child.wait());
    match line.trim_end() {
        SHOWN | NO_SERVER => Ok(()),
        "" => Err("the notifier died".into()),
        why => Err(why.to_owned()),
    }
}

/// The child: send the notification, report, then act on clicks.
pub fn serve_child(args: NotifyArgs) -> Result<(), (Exit, String)> {
    std::thread::spawn(|| {
        std::thread::sleep(LONGEST_WAIT);
        std::process::exit(0);
    });
    zbus::block_on(serve(args)).map_err(|e| (Exit::Io, e.to_string()))
}

fn report(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

async fn serve(args: NotifyArgs) -> zbus::Result<()> {
    let Ok(conn) = zbus::Connection::session().await else {
        report(NO_SERVER);
        return Ok(());
    };
    let proxy = NotificationsProxy::new(&conn).await?;
    // Subscribe before notifying, so not even a very quick click is missed.
    let mut actions = proxy.receive_action_invoked().await?;
    let mut closed = proxy.receive_notification_closed().await?;

    let summary = if args.path.is_some() {
        "Screenshot saved"
    } else {
        "Screenshot taken"
    };
    let mut body = format!("{} × {}", args.width, args.height);
    for badge in &args.badges {
        body = format!("{body} · {badge}");
    }
    let mut hints = HashMap::from([("desktop-entry", Variant::from("hoarder"))]);
    let mut buttons: Vec<&str> = Vec::new();
    let uri = args.path.as_deref().map(file_uri);
    if let (Some(path), Some(uri)) = (&args.path, &uri) {
        if let Some(name) = path.file_name() {
            body = format!("{} · {body}", name.to_string_lossy());
        }
        hints.insert("image-path", Variant::from(uri.as_str()));
        buttons = vec![
            "default",
            "Open",
            "open",
            "Open",
            "edit",
            "Edit",
            "folder",
            "Show in folder",
            "delete",
            "Delete",
        ];
    }
    let id = match proxy
        .notify(
            "Hoarder",
            0,
            "camera-photo",
            summary,
            &body,
            &buttons,
            hints,
            -1,
        )
        .await
    {
        Ok(id) => id,
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            report(NO_SERVER);
            return Ok(());
        }
        Err(e) => {
            report(&e.to_string());
            return Ok(());
        }
    };
    report(SHOWN);
    let (Some(path), Some(uri)) = (args.path, uri) else {
        return Ok(());
    };

    loop {
        let next = futures_util::future::select(actions.next(), closed.next()).await;
        match next {
            futures_util::future::Either::Left((Some(signal), _)) => {
                let Ok(a) = signal.args() else { continue };
                if a.id != id {
                    continue;
                }
                match a.action_key.as_str() {
                    "default" | "open" => run_detached("xdg-open", &[path.as_os_str()]),
                    "edit" => edit(&path),
                    "folder" => show_in_folder(&conn, &path, &uri).await,
                    "delete" => {
                        if let Err(e) = std::fs::remove_file(&path) {
                            eprintln!("hoarder: cannot delete {}: {e}", path.display());
                        }
                        let _ = proxy.close_notification(id).await;
                        return Ok(());
                    }
                    _ => {}
                }
            }
            futures_util::future::Either::Right((Some(signal), _)) => {
                if signal.args().is_ok_and(|a| a.id == id) {
                    return Ok(());
                }
            }
            // The bus went away.
            _ => return Ok(()),
        }
    }
}

/// Open in an annotation tool if one is installed, else the default app.
fn edit(path: &Path) {
    let editors: [(&str, &str); 2] = [("satty", "--filename"), ("swappy", "-f")];
    match editors.iter().find(|(name, _)| on_path(name)) {
        Some((name, flag)) => run_detached(name, &[flag.as_ref(), path.as_os_str()]),
        None => run_detached("xdg-open", &[path.as_os_str()]),
    }
}

async fn show_in_folder(conn: &zbus::Connection, path: &Path, uri: &str) {
    let shown = match FileManager1Proxy::new(conn).await {
        Ok(fm) => fm.show_items(&[uri], "").await.is_ok(),
        Err(_) => false,
    };
    if !shown && let Some(dir) = path.parent() {
        run_detached("xdg-open", &[dir.as_os_str()]);
    }
}

/// `file://` URI for an absolute path, percent-encoding all but RFC 3986
/// unreserved characters and `/`.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}
