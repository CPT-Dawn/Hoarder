//! The Capture notification (PLAN.md §4.1 step 7): a thumbnail plus Open,
//! Edit, Show in folder and Delete, over org.freedesktop.Notifications.
//!
//! Each test runs its own `dbus-daemon` at `$XDG_RUNTIME_DIR/bus` inside the
//! Sway session's private directory, with a stand-in notification server on
//! it. `Session::command` clears the environment, so `hoarder` can only reach
//! that bus: nothing here ever shows up on the real desktop.

use std::collections::HashMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hoarder_e2e::{OutputSpec, Session};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedVariant;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// One `Notify` call the server received.
#[derive(Debug, Clone)]
struct Notified {
    app_name: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: HashMap<String, String>,
}

#[derive(Debug, Default)]
struct Log {
    notified: Vec<Notified>,
    closed: Vec<u32>,
}

#[derive(Debug)]
struct Server {
    log: Arc<Mutex<Log>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        ["actions", "body", "icon-static"]
            .map(String::from)
            .to_vec()
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "stand-in".into(),
            "hoarder".into(),
            "0".into(),
            "1.2".into(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        _expire_timeout: i32,
    ) -> u32 {
        let hints = hints
            .into_iter()
            .map(|(k, v)| {
                let v = String::try_from(v).unwrap_or_else(|v| format!("{v:?}"));
                (k, v)
            })
            .collect();
        let mut log = self.log.lock().unwrap();
        log.notified.push(Notified {
            app_name,
            summary,
            body,
            actions,
            hints,
        });
        log.notified.len() as u32
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.log.lock().unwrap().closed.push(id);
        Self::notification_closed(&emitter, id, 3).await?;
        Ok(())
    }

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

/// Lets the test click a notification's button: the signal has to come from
/// the server's own connection, or `hoarder` (rightly) ignores it.
#[derive(Debug)]
struct Remote;

#[zbus::interface(name = "org.hoarder.Test")]
impl Remote {
    async fn invoke(
        &self,
        id: u32,
        key: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        Server::action_invoked(&emitter, id, &key).await?;
        Ok(())
    }
}

/// A private `dbus-daemon` and a notification server on it.
struct Bus {
    daemon: Child,
    address: String,
    log: Arc<Mutex<Log>>,
}

impl Bus {
    fn start(session: &Session) -> Bus {
        let dir = session.runtime_dir();
        let socket = dir.join("bus");
        // No <servicedir>: nothing may be D-Bus-activated (a real mako, say).
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                socket.display()
            ),
        )
        .unwrap();
        let mut daemon = Command::new("dbus-daemon")
            .args(["--nofork", "--nopidfile", "--print-address"])
            .arg(format!("--config-file={}", config.display()))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon should be installed");
        let mut address = String::new();
        std::io::BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon printed no address");

        let log = Arc::new(Mutex::new(Log::default()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let server_log = log.clone();
        let server_address = address.clone();
        std::thread::spawn(move || {
            zbus::block_on(async move {
                let conn = zbus::connection::Builder::address(server_address.as_str())
                    .serve_at(PATH, Server { log: server_log })
                    .serve_at(PATH, Remote)
                    .name(NAME)
                    .build()
                    .await
                    .unwrap();
                ready_tx.send(()).unwrap();
                // Serve until the test process ends.
                std::future::pending::<()>().await;
                drop(conn);
            })
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("notification server up");
        Bus {
            daemon,
            address,
            log,
        }
    }

    /// Wait for the `n`th notification (1-based) and return it.
    fn notified(&self, n: usize) -> Notified {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(n) = self.log.lock().unwrap().notified.get(n - 1) {
                return n.clone();
            }
            assert!(Instant::now() < deadline, "no notification arrived");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn count(&self) -> usize {
        self.log.lock().unwrap().notified.len()
    }

    fn closed(&self) -> Vec<u32> {
        self.log.lock().unwrap().closed.clone()
    }

    /// Click `key` on notification `id`, as the user would.
    fn invoke(&self, id: u32, key: &str) {
        zbus::block_on(async {
            let conn = zbus::connection::Builder::address(self.address.as_str())
                .build()
                .await
                .unwrap();
            conn.call_method(
                Some(NAME),
                PATH,
                Some("org.hoarder.Test"),
                "Invoke",
                &(id, key),
            )
            .await
            .unwrap();
        });
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// A directory of stand-ins for `xdg-open` and friends, each appending its
/// name and arguments to `<dir>/calls`.
fn fake_tools(session: &Session, names: &[&str]) -> PathBuf {
    let dir = session.runtime_dir().join("fake-bin");
    std::fs::create_dir_all(&dir).unwrap();
    for name in names {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{name} $*\" >> {}\n",
                dir.join("calls").display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    dir
}

/// Wait for a line in `<fake>/calls` starting with `prefix`.
fn wait_for_call(fake: &Path, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let calls = std::fs::read_to_string(fake.join("calls")).unwrap_or_default();
        if let Some(line) = calls.lines().find(|l| l.starts_with(prefix)) {
            return line.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "never called {prefix:?}: {calls:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn hoarder(session: &Session, fake: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = session.command(env!("CARGO_BIN_EXE_hoarder"));
    if let Some(fake) = fake {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{path}", fake.display()));
    }
    let out = cmd.args(args).output().expect("hoarder should run");
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn saved_path(out: &Output) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim_end())
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_capture_is_announced_with_a_thumbnail_and_four_actions() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);

    let out = hoarder(&session, None, &["shot", "all"]);

    let saved = saved_path(&out);
    let n = bus.notified(1);
    assert_eq!(n.app_name, "Hoarder");
    assert!(n.summary.contains("Screenshot"), "summary: {}", n.summary);
    assert!(n.body.contains("320 × 240"), "body: {}", n.body);
    let keys: Vec<&str> = n.actions.iter().step_by(2).map(String::as_str).collect();
    let labels: Vec<&str> = n
        .actions
        .iter()
        .skip(1)
        .step_by(2)
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["default", "open", "edit", "folder", "delete"]);
    assert_eq!(labels[1..], ["Open", "Edit", "Show in folder", "Delete"]);
    assert_eq!(
        n.hints.get("image-path").map(String::as_str),
        Some(format!("file://{}", saved.display()).as_str())
    );
}

#[test]
fn no_notify_flag_keeps_quiet() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);

    hoarder(&session, None, &["shot", "all", "--no-notify"]);
    hoarder(
        &session,
        None,
        &["--set", "screenshot.notify=false", "shot", "all"],
    );

    // A notification would have arrived well within this.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(bus.count(), 0);
}

#[test]
fn delete_removes_the_file_and_closes_the_notification() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);

    let saved = saved_path(&hoarder(&session, None, &["shot", "all"]));
    bus.notified(1);
    assert!(saved.is_file());
    bus.invoke(1, "delete");

    wait_until("the file is deleted", || !saved.exists());
    wait_until("the notification is closed", || bus.closed() == [1]);
}

#[test]
fn open_and_clicking_the_body_open_the_capture() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);
    let fake = fake_tools(&session, &["xdg-open"]);

    let saved = saved_path(&hoarder(&session, Some(&fake), &["shot", "all"]));
    bus.notified(1);
    bus.invoke(1, "open");

    assert_eq!(
        wait_for_call(&fake, "xdg-open"),
        format!("xdg-open {}", saved.display())
    );
}

#[test]
fn edit_prefers_an_annotation_tool() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);
    let fake = fake_tools(&session, &["xdg-open", "satty"]);

    let saved = saved_path(&hoarder(&session, Some(&fake), &["shot", "all"]));
    bus.notified(1);
    bus.invoke(1, "edit");

    assert_eq!(
        wait_for_call(&fake, "satty"),
        format!("satty --filename {}", saved.display())
    );
}

#[test]
fn show_in_folder_falls_back_to_opening_the_directory() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);
    let fake = fake_tools(&session, &["xdg-open"]);

    let saved = saved_path(&hoarder(&session, Some(&fake), &["shot", "all"]));
    bus.notified(1);
    // The private bus has no org.freedesktop.FileManager1.
    bus.invoke(1, "folder");

    assert_eq!(
        wait_for_call(&fake, "xdg-open"),
        format!("xdg-open {}", saved.parent().unwrap().display())
    );
}

#[test]
fn a_capture_to_stdout_is_announced_without_file_actions() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();
    let bus = Bus::start(&session);

    hoarder(&session, None, &["shot", "all", "-o", "-"]);

    let n = bus.notified(1);
    assert!(n.actions.is_empty(), "actions: {:?}", n.actions);
    assert!(!n.hints.contains_key("image-path"));
}

#[test]
fn shot_is_silent_and_successful_without_a_notification_server() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();

    let out = hoarder(&session, None, &["shot", "all"]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("notif"), "stderr: {stderr}");
}
