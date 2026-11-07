//! A private `dbus-daemon` with a stand-in notification server, for tests
//! that check what Hoarder announces.
//!
//! The bus listens at `$XDG_RUNTIME_DIR/bus` inside the Sway session's
//! private directory. `Session::command` clears the environment, so `hoarder`
//! can only reach that bus: nothing here ever shows up on the real desktop.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::BufRead as _;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hoarder_e2e::Session;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedVariant;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// One `Notify` call the server received.
#[derive(Debug, Clone)]
pub struct Notified {
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub hints: HashMap<String, String>,
    /// The `image-data` hint, if it was a well-formed one.
    pub image: Option<ImageData>,
}

/// An `image-data` hint: raw pixels the server shows as the icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub bits_per_sample: i32,
    pub channels: i32,
    pub data: Vec<u8>,
}

impl ImageData {
    fn parse(v: OwnedVariant) -> Option<ImageData> {
        let s = zbus::zvariant::Structure::try_from(v).ok()?;
        let (width, height, rowstride, has_alpha, bits_per_sample, channels, data) =
            <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(s).ok()?;
        Some(ImageData {
            width,
            height,
            rowstride,
            has_alpha,
            bits_per_sample,
            channels,
            data,
        })
    }
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
        let mut hints = hints;
        let image = hints.remove("image-data").and_then(ImageData::parse);
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
            image,
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
pub struct Bus {
    daemon: Child,
    address: String,
    log: Arc<Mutex<Log>>,
}

impl Bus {
    pub fn start(session: &Session) -> Bus {
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
    pub fn notified(&self, n: usize) -> Notified {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(n) = self.log.lock().unwrap().notified.get(n - 1) {
                return n.clone();
            }
            assert!(Instant::now() < deadline, "no notification arrived");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn count(&self) -> usize {
        self.log.lock().unwrap().notified.len()
    }

    pub fn closed(&self) -> Vec<u32> {
        self.log.lock().unwrap().closed.clone()
    }

    /// Click `key` on notification `id`, as the user would.
    pub fn invoke(&self, id: u32, key: &str) {
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
