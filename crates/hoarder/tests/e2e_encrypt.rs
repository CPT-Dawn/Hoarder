//! Encryption at rest (`history.encrypt`, PLAN.md §5.3): History sealed with
//! age, its key in the Secret Service keyring.
//!
//! Each test runs its own `dbus-daemon` at `$XDG_RUNTIME_DIR/bus` with a
//! stand-in Secret Service on it. `Session::command` clears the environment,
//! so `hoarder` can only reach that bus: no test key ever lands in the real
//! keyring. Observed through the CLI, wl-clipboard, the stand-in keyring and
//! the raw bytes on disk.

use std::collections::HashMap;
use std::io::BufRead as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hoarder_e2e::{Selection, Session};
use serde_json::Value;
use zbus::object_server::ObjectServer;
use zbus::{OwnedObjectPath, OwnedVariant, Str, Variant};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

// ---------------------------------------------------------------------------
// A stand-in Secret Service (org.freedesktop.secrets), plain sessions only.

const SERVICE: &str = "/org/freedesktop/secrets";
const SESSION: &str = "/org/freedesktop/secrets/session/s1";
const LOGIN: &str = "/org/freedesktop/secrets/collection/login";

#[derive(Debug, Clone)]
struct StoredItem {
    label: String,
    attributes: HashMap<String, String>,
    secret: Vec<u8>,
}

/// The keyring's contents, shared with the test.
#[derive(Debug, Default)]
struct Keyring {
    locked: bool,
    items: Vec<StoredItem>,
}

type Shared = Arc<Mutex<Keyring>>;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.Secret.Error")]
enum SecretError {
    #[zbus(error)]
    ZBus(zbus::Error),
    IsLocked(String),
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.hoarder.StandIn.Error")]
enum StandInError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotSupported(String),
}

#[derive(Debug, serde::Serialize, zbus::Type)]
struct DBusSecret(OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn path(p: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p).unwrap()
}

fn item_path(n: usize) -> OwnedObjectPath {
    path(&format!("{LOGIN}/{n}"))
}

#[derive(Debug)]
struct Service;

#[zbus::interface(name = "org.freedesktop.Secret.Service")]
impl Service {
    fn open_session(
        &self,
        algorithm: &str,
        _input: OwnedVariant,
    ) -> Result<(OwnedVariant, OwnedObjectPath), StandInError> {
        // Not an org.freedesktop.DBus.Error name: oo7 falls back to a plain
        // session only on errors zbus does not map to its fdo set.
        if algorithm != "plain" {
            return Err(StandInError::NotSupported(algorithm.into()));
        }
        let empty = OwnedVariant::try_from(Variant::from(Str::from(""))).unwrap();
        Ok((empty, path(SESSION)))
    }

    fn read_alias(&self, name: &str) -> OwnedObjectPath {
        path(if name == "default" { LOGIN } else { "/" })
    }
}

#[derive(Debug)]
struct SessionObject;

#[zbus::interface(name = "org.freedesktop.Secret.Session")]
impl SessionObject {
    fn close(&self) {}
}

#[derive(Debug)]
struct Collection {
    keyring: Shared,
}

#[zbus::interface(name = "org.freedesktop.Secret.Collection")]
impl Collection {
    #[zbus(property)]
    fn locked(&self) -> bool {
        self.keyring.lock().unwrap().locked
    }

    #[zbus(property)]
    fn label(&self) -> String {
        "Login".into()
    }

    #[zbus(property)]
    fn items(&self) -> Vec<OwnedObjectPath> {
        let n = self.keyring.lock().unwrap().items.len();
        (0..n).map(item_path).collect()
    }

    #[zbus(property)]
    fn created(&self) -> u64 {
        0
    }

    #[zbus(property)]
    fn modified(&self) -> u64 {
        0
    }

    fn search_items(&self, attributes: HashMap<String, String>) -> Vec<OwnedObjectPath> {
        let keyring = self.keyring.lock().unwrap();
        (0..keyring.items.len())
            .filter(|&n| {
                attributes
                    .iter()
                    .all(|(k, v)| keyring.items[n].attributes.get(k) == Some(v))
            })
            .map(item_path)
            .collect()
    }

    async fn create_item(
        &self,
        properties: HashMap<String, OwnedVariant>,
        secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
        _replace: bool,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath), SecretError> {
        let attributes = properties
            .get("org.freedesktop.Secret.Item.Attributes")
            .map(|v| HashMap::<String, String>::try_from(v.try_clone().unwrap()).unwrap())
            .unwrap_or_default();
        let label = properties
            .get("org.freedesktop.Secret.Item.Label")
            .map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
            .unwrap_or_default();
        let n = {
            let mut keyring = self.keyring.lock().unwrap();
            if keyring.locked {
                return Err(SecretError::IsLocked("collection is locked".into()));
            }
            keyring.items.push(StoredItem {
                label,
                attributes,
                secret: secret.2,
            });
            keyring.items.len() - 1
        };
        let item = Item {
            keyring: self.keyring.clone(),
            n,
        };
        server.at(item_path(n), item).await?;
        Ok((item_path(n), path("/")))
    }
}

#[derive(Debug)]
struct Item {
    keyring: Shared,
    n: usize,
}

#[zbus::interface(name = "org.freedesktop.Secret.Item")]
impl Item {
    #[zbus(property)]
    fn locked(&self) -> bool {
        self.keyring.lock().unwrap().locked
    }

    #[zbus(property)]
    fn label(&self) -> String {
        self.keyring.lock().unwrap().items[self.n].label.clone()
    }

    #[zbus(property)]
    fn attributes(&self) -> HashMap<String, String> {
        self.keyring.lock().unwrap().items[self.n]
            .attributes
            .clone()
    }

    #[zbus(property)]
    fn created(&self) -> u64 {
        0
    }

    #[zbus(property)]
    fn modified(&self) -> u64 {
        0
    }

    fn get_secret(&self, _session: OwnedObjectPath) -> Result<DBusSecret, SecretError> {
        let keyring = self.keyring.lock().unwrap();
        if keyring.locked {
            return Err(SecretError::IsLocked("item is locked".into()));
        }
        Ok(DBusSecret(
            path(SESSION),
            Vec::new(),
            keyring.items[self.n].secret.clone(),
            "text/plain".into(),
        ))
    }
}

/// A private `dbus-daemon`, optionally with the stand-in Secret Service.
struct Bus {
    daemon: Child,
    keyring: Shared,
}

impl Bus {
    /// A bus with a Secret Service holding `keyring`.
    fn with_keyring(session: &Session, keyring: Keyring) -> Bus {
        let bus = Bus::bare(session);
        *bus.keyring.lock().unwrap() = keyring;
        let address = format!("unix:path={}", session.runtime_dir().join("bus").display());
        let shared = bus.keyring.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            zbus::block_on(async move {
                let items: Vec<Item> = (0..shared.lock().unwrap().items.len())
                    .map(|n| Item {
                        keyring: shared.clone(),
                        n,
                    })
                    .collect();
                let mut builder = zbus::connection::Builder::address(address.as_str())
                    .serve_at(SERVICE, Service)
                    .serve_at(SESSION, SessionObject)
                    .serve_at(LOGIN, Collection { keyring: shared });
                for (n, item) in items.into_iter().enumerate() {
                    builder = builder.serve_at(item_path(n), item);
                }
                let conn = builder
                    .name("org.freedesktop.secrets")
                    .build()
                    .await
                    .unwrap();
                ready_tx.send(()).unwrap();
                std::future::pending::<()>().await;
                drop(conn);
            })
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("Secret Service up");
        bus
    }

    /// A bus with nobody on it.
    fn bare(session: &Session) -> Bus {
        let dir = session.runtime_dir();
        let socket = dir.join("bus");
        let _ = std::fs::remove_file(&socket);
        // No <servicedir>: nothing may be D-Bus-activated (a real keyring, say).
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
        assert!(!address.trim().is_empty(), "dbus-daemon printed no address");
        Bus {
            daemon,
            keyring: Shared::default(),
        }
    }

    fn items(&self) -> Vec<StoredItem> {
        self.keyring.lock().unwrap().items.clone()
    }

    fn set_locked(&self, locked: bool) {
        self.keyring.lock().unwrap().locked = locked;
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

// ---------------------------------------------------------------------------
// The Daemon and the CLI.

/// A running `hoarder daemon`, killed when dropped.
struct Daemon(Child);

impl Daemon {
    fn start(session: &Session) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let child = session
            .command(HOARDER)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start hoarder daemon");
        let mut daemon = Daemon(child);
        let socket = session.runtime_dir().join("hoarder/hoarder.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&socket).is_err() {
            if let Ok(Some(status)) = daemon.0.try_wait() {
                panic!("daemon exited early ({status}): {}", daemon_log(session));
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    /// Stop it the way systemd does, so SQLite closes cleanly.
    fn stop(mut self) {
        let pid = rustix::process::Pid::from_child(&self.0);
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "daemon ignored SIGTERM");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn daemon_log(session: &Session) -> String {
    std::fs::read_to_string(session.runtime_dir().join("daemon.log")).unwrap_or_default()
}

fn wait_until(what: &str, session: &Session, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; daemon log:\n{}",
            daemon_log(session)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(HOARDER)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// `hoarder history list --json`, or its error message.
fn try_history(session: &Session) -> Result<Vec<Value>, String> {
    let out = hoarder(session, &["history", "list", "--json"]);
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(serde_json::from_slice::<Value>(&out.stdout)
        .unwrap()
        .as_array()
        .expect("a JSON array")
        .clone())
}

fn previews(session: &Session) -> Vec<String> {
    try_history(session)
        .unwrap_or_else(|e| panic!("history list failed: {e}"))
        .iter()
        .map(|c| c["preview"].as_str().unwrap().to_owned())
        .collect()
}

/// Copy `text` from a fresh app, which keeps serving it until dropped.
fn copy(session: &Session, text: &[u8]) -> hoarder_e2e::Client {
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", text)])
        .unwrap();
    app
}

fn set_config(session: &Session, toml: &str) {
    let dir = session.runtime_dir().join("home/.config/hoarder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

const ENCRYPT: &str = "[history]\nencrypt = true\n";
const PLAIN: &str = "[history]\nencrypt = false\n";

fn data_dir(session: &Session) -> PathBuf {
    session.runtime_dir().join("home/.local/share/hoarder")
}

/// Whether `needle` appears anywhere in the History's files.
fn on_disk(dir: &Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.map(|e| e.unwrap().path()).any(|path| {
        if path.is_dir() {
            on_disk(&path, needle)
        } else {
            std::fs::read(&path)
                .unwrap_or_default()
                .windows(needle.len())
                .any(|w| w == needle)
        }
    })
}

// ---------------------------------------------------------------------------

#[test]
fn an_encrypted_history_is_unreadable_on_disk_and_works_as_usual() {
    let session = Session::start_default().unwrap();
    let bus = Bus::with_keyring(&session, Keyring::default());
    set_config(&session, ENCRYPT);
    let daemon = Daemon::start(&session);

    let _app = copy(&session, b"e2e vault code 7731");
    wait_until("the Clip", &session, || {
        previews(&session) == ["e2e vault code 7731"]
    });
    let items = bus.items();
    assert_eq!(items.len(), 1, "one key in the keyring: {items:?}");
    assert!(items[0].secret.starts_with(b"AGE-SECRET-KEY-1"));
    assert!(!on_disk(&data_dir(&session), b"vault code"));

    // A restart fetches the key again and reads the History back.
    daemon.stop();
    assert!(!on_disk(&data_dir(&session), b"vault code"));
    let _daemon = Daemon::start(&session);
    assert_eq!(previews(&session), ["e2e vault code 7731"]);
    assert_eq!(bus.items().len(), 1, "the key is reused, not replaced");
}

#[test]
fn turning_encryption_on_and_off_live_migrates_history_both_ways() {
    let session = Session::start_default().unwrap();
    let _bus = Bus::with_keyring(&session, Keyring::default());
    set_config(&session, PLAIN);
    let _daemon = Daemon::start(&session);
    let _app = copy(&session, b"migrate me 4412");
    wait_until("the Clip", &session, || {
        previews(&session) == ["migrate me 4412"]
    });
    let dir = data_dir(&session);
    wait_until("plain History on disk", &session, || {
        on_disk(&dir, b"migrate me")
    });

    set_config(&session, ENCRYPT);
    wait_until("the History to be sealed", &session, || {
        !on_disk(&dir, b"migrate me")
    });
    assert_eq!(previews(&session), ["migrate me 4412"]);

    set_config(&session, PLAIN);
    wait_until("the History to be plain again", &session, || {
        on_disk(&dir, b"migrate me")
    });
    assert_eq!(previews(&session), ["migrate me 4412"]);
}

#[test]
fn a_locked_keyring_is_named_and_history_opens_once_it_is_unlocked() {
    let session = Session::start_default().unwrap();
    let bus = Bus::with_keyring(
        &session,
        Keyring {
            locked: true,
            ..Keyring::default()
        },
    );
    set_config(&session, ENCRYPT);
    let _daemon = Daemon::start(&session);

    let err = try_history(&session).expect_err("History must be unavailable");
    assert!(err.contains("keyring is locked"), "{err}");
    let _app = copy(&session, b"never in plain 9921");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!on_disk(&data_dir(&session), b"never in plain"));

    bus.set_locked(false);
    let _again = copy(&session, b"after unlock 5150");
    wait_until("History to open", &session, || {
        try_history(&session).is_ok_and(|c| c.len() == 1)
    });
    assert_eq!(previews(&session), ["after unlock 5150"]);
    assert!(!on_disk(&data_dir(&session), b"after unlock"));
}

#[test]
fn no_secret_service_is_named() {
    let session = Session::start_default().unwrap();
    let _bus = Bus::bare(&session);
    set_config(&session, ENCRYPT);
    let _daemon = Daemon::start(&session);

    let err = try_history(&session).expect_err("History must be unavailable");
    assert!(err.contains("no Secret Service"), "{err}");
}

#[test]
fn a_key_missing_from_the_keyring_is_named_and_nothing_is_lost() {
    let session = Session::start_default().unwrap();
    let bus = Bus::with_keyring(&session, Keyring::default());
    set_config(&session, ENCRYPT);
    let daemon = Daemon::start(&session);
    let _app = copy(&session, b"keep me 3141");
    wait_until("the Clip", &session, || previews(&session).len() == 1);
    daemon.stop();
    let items = bus.items();
    drop(bus);

    // The keyring was wiped: say so, and make no new key over the old one.
    let bus = Bus::with_keyring(&session, Keyring::default());
    let daemon = Daemon::start(&session);
    let err = try_history(&session).expect_err("History must be unavailable");
    assert!(err.contains("missing from the keyring"), "{err}");
    assert!(bus.items().is_empty(), "a new key would orphan the History");
    daemon.stop();
    drop(bus);

    // Put the key back and the History is all there.
    let _bus = Bus::with_keyring(
        &session,
        Keyring {
            locked: false,
            items,
        },
    );
    let _daemon = Daemon::start(&session);
    assert_eq!(previews(&session), ["keep me 3141"]);
}
