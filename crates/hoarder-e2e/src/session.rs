//! A throwaway headless Sway session, isolated from the user's desktop.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Error, Input, Result};

/// How long to wait for Sway to come up before giving up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Bit depth of an output's render buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    /// `XRGB8888`.
    Eight,
    /// `XRGB2101010`.
    Ten,
}

/// Output rotation as written in Sway's config (`output … transform 90`).
///
/// Careful: Sway's names are the inverse of the protocol's. `Rotate90` here
/// makes Sway advertise `wl_output.transform = 270` (and vice versa), so a
/// raw capture must be rotated clockwise by the *Sway* angle to be upright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    fn sway_name(self) -> &'static str {
        match self {
            Transform::Normal => "normal",
            Transform::Rotate90 => "90",
            Transform::Rotate180 => "180",
            Transform::Rotate270 => "270",
            Transform::Flipped => "flipped",
            Transform::Flipped90 => "flipped-90",
            Transform::Flipped180 => "flipped-180",
            Transform::Flipped270 => "flipped-270",
        }
    }

    fn swaps_axes(self) -> bool {
        matches!(
            self,
            Transform::Rotate90
                | Transform::Rotate270
                | Transform::Flipped90
                | Transform::Flipped270
        )
    }
}

/// One headless output to create.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputSpec {
    /// Mode width in physical pixels (before the transform).
    pub width: u32,
    /// Mode height in physical pixels (before the transform).
    pub height: u32,
    pub scale: f64,
    pub transform: Transform,
    pub depth: BitDepth,
}

impl OutputSpec {
    /// A plain 8-bit, unscaled, unrotated output.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            scale: 1.0,
            transform: Transform::Normal,
            depth: BitDepth::Eight,
        }
    }

    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    pub fn transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    pub fn depth(mut self, depth: BitDepth) -> Self {
        self.depth = depth;
        self
    }

    /// Size in physical pixels after the transform, i.e. as the user sees it.
    pub fn physical_size(&self) -> (u32, u32) {
        if self.transform.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Size in logical (layout) pixels.
    pub fn logical_size(&self) -> (u32, u32) {
        let (w, h) = self.physical_size();
        (
            (w as f64 / self.scale).round() as u32,
            (h as f64 / self.scale).round() as u32,
        )
    }
}

/// An output of a running [`Session`].
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// Connector name, `HEADLESS-1` and up, in the order the specs were given.
    pub name: String,
    pub spec: OutputSpec,
    /// Position of the top-left corner in the layout (logical pixels).
    pub position: (i32, i32),
}

/// A headless Sway session (`WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`).
///
/// Everything lives in a private temporary directory: the Wayland and Sway IPC
/// sockets, and a fake `HOME` with its own XDG directories, so nothing a test
/// runs can touch the real desktop, config or History. Dropping the session
/// kills Sway and removes the directory.
#[derive(Debug)]
pub struct Session {
    sway: Child,
    dir: tempfile::TempDir,
    wayland_display: String,
    swaysock: PathBuf,
    outputs: Vec<Output>,
    input: Option<Input>,
}

impl Session {
    /// A session with a single 640×480, 8-bit output.
    pub fn start_default() -> Result<Self> {
        Self::start(&[OutputSpec::new(640, 480)])
    }

    /// Start Sway with one headless output per spec, laid out left to right.
    pub fn start(specs: &[OutputSpec]) -> Result<Self> {
        assert!(!specs.is_empty(), "a session needs at least one output");

        // Wayland socket paths are capped at 108 bytes, so keep the directory short.
        let dir = tempfile::Builder::new()
            .prefix("hoarder-e2e-")
            .tempdir_in("/tmp")?;
        let home = dir.path().join("home");
        std::fs::create_dir(&home)?;

        let mut outputs = Vec::with_capacity(specs.len());
        let mut config = String::from("xwayland disable\ndefault_border none\n");
        let mut x = 0i32;
        for (i, spec) in specs.iter().enumerate() {
            let name = format!("HEADLESS-{}", i + 1);
            let depth = match spec.depth {
                BitDepth::Eight => 8,
                BitDepth::Ten => 10,
            };
            writeln!(
                config,
                "output {name} mode {}x{} position {x} 0 scale {} transform {} render_bit_depth {depth}",
                spec.width,
                spec.height,
                spec.scale,
                spec.transform.sway_name()
            )
            .expect("writing to a String cannot fail");
            outputs.push(Output {
                name,
                spec: *spec,
                position: (x, 0),
            });
            x += spec.logical_size().0 as i32;
        }
        let config_path = dir.path().join("sway.conf");
        std::fs::write(&config_path, config)?;

        let log = std::fs::File::create(dir.path().join("sway.log"))?;
        let sway = Command::new("sway")
            .args(std::env::var_os("HOARDER_E2E_KEEP").map(|_| "--debug"))
            .arg("--config")
            .arg(&config_path)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", dir.path())
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("WLR_HEADLESS_OUTPUTS", specs.len().to_string())
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .map_err(|e| Error::Setup(format!("cannot start sway (is it installed?): {e}")))?;

        let mut session = Session {
            sway,
            dir,
            wayland_display: String::new(),
            swaysock: PathBuf::new(),
            outputs,
            input: None,
        };
        session.wait_until_ready()?;
        // Created up front so every client connected later already sees a
        // pointer and keyboard on the seat; no capability races in tests.
        session.input = Some(Input::new(session.client()?, &session.outputs)?);
        Ok(session)
    }

    fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = self.sway.try_wait()? {
                return Err(Error::Setup(format!(
                    "sway exited early ({status}):\n{}",
                    self.log_tail()
                )));
            }
            if let Some((display, sock)) = self.find_sockets()? {
                self.wayland_display = display;
                self.swaysock = sock;
                if self.outputs_are_active() {
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                return Err(Error::Setup(format!(
                    "sway did not come up:\n{}",
                    self.log_tail()
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn find_sockets(&self) -> Result<Option<(String, PathBuf)>> {
        let mut display = None;
        let mut sock = None;
        for entry in std::fs::read_dir(self.dir.path())? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("wayland-") && !name.ends_with(".lock") {
                display = Some(name);
            } else if name.starts_with("sway-ipc.") && name.ends_with(".sock") {
                sock = Some(self.dir.path().join(name));
            }
        }
        Ok(display.zip(sock))
    }

    fn outputs_are_active(&self) -> bool {
        let Ok(json) = self.swaymsg(&["-t", "get_outputs", "--raw"]) else {
            return false;
        };
        self.outputs
            .iter()
            .all(|o| json.contains(&format!("\"name\": \"{}\"", o.name)))
            && json.matches("\"active\": true").count() >= self.outputs.len()
    }

    fn log_tail(&self) -> String {
        let log = std::fs::read_to_string(self.dir.path().join("sway.log")).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(30)..].join("\n")
    }

    /// The outputs, in the order their specs were given.
    pub fn outputs(&self) -> &[Output] {
        &self.outputs
    }

    /// Look up an output by connector name.
    pub fn output(&self, name: &str) -> &Output {
        self.outputs
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("no output named {name}"))
    }

    /// The session's virtual pointer and keyboard.
    pub fn input(&self) -> &Input {
        self.input.as_ref().expect("created in Session::start")
    }

    /// `XDG_RUNTIME_DIR` of the session (also where its sockets live).
    pub fn runtime_dir(&self) -> &Path {
        self.dir.path()
    }

    /// Value of `WAYLAND_DISPLAY` inside the session.
    pub fn wayland_display(&self) -> &str {
        &self.wayland_display
    }

    /// A command whose environment points at this session and nowhere else:
    /// a fresh `HOME`, private XDG directories, and no trace of the caller's
    /// `WAYLAND_DISPLAY`, `DISPLAY` or Hyprland instance.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let program = program.as_ref();
        if program == "wl-copy" || program == "wl-paste" {
            require_wl_clipboard();
        }
        let home = self.dir.path().join("home");
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LANG", "C.UTF-8")
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", self.dir.path())
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_CURRENT_DESKTOP", "sway")
            .env("WAYLAND_DISPLAY", &self.wayland_display)
            .env("SWAYSOCK", &self.swaysock);
        if let Some(v) = std::env::var_os("RUST_BACKTRACE") {
            cmd.env("RUST_BACKTRACE", v);
        }
        cmd
    }

    /// Run `swaymsg` against this session and return its stdout.
    pub fn swaymsg(&self, args: &[&str]) -> Result<String> {
        let out = self.command("swaymsg").args(args).output()?;
        if !out.status.success() {
            return Err(Error::Command(format!(
                "swaymsg {args:?} failed: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.sway.kill();
        let _ = self.sway.wait();
        if std::env::var_os("HOARDER_E2E_KEEP").is_some() {
            self.dir.disable_cleanup(true);
            eprintln!(
                "hoarder-e2e: kept session directory {}",
                self.dir.path().display()
            );
        }
    }
}

/// Whether `wl-copy --version` printed this came from wl-clipboard itself.
/// `hoarder-wl-compat` puts Hoarder behind the same names (`wl-copy 1.0.1`).
fn is_wl_clipboard(version: &str) -> bool {
    version.starts_with("wl-clipboard ")
}

/// The tests use `wl-copy`/`wl-paste` as an independent oracle, so Hoarder
/// standing in for them (hoarder-wl-compat) would test Hoarder against
/// itself. Stop with a clear message instead of a puzzling failure.
fn require_wl_clipboard() {
    static CHECK: std::sync::OnceLock<std::result::Result<(), String>> = std::sync::OnceLock::new();
    let check = CHECK.get_or_init(|| {
        let out = Command::new("wl-copy")
            .arg("--version")
            .output()
            .map_err(|e| format!("cannot run wl-copy: {e}"))?;
        let version = String::from_utf8_lossy(&out.stdout);
        if is_wl_clipboard(&version) {
            Ok(())
        } else {
            Err(format!(
                "wl-copy on PATH is not wl-clipboard (it says {:?}; is hoarder-wl-compat \
                 installed?). The e2e tests need the real one as an oracle: put \
                 wl-clipboard's binaries first on PATH (docs/testing.md)",
                version.trim()
            ))
        }
    });
    if let Err(e) = check {
        panic!("{e}");
    }
}

#[cfg(test)]
mod wl_clipboard_tests {
    use super::is_wl_clipboard;

    #[test]
    fn tells_wl_clipboard_from_the_hoarder_shim() {
        assert!(is_wl_clipboard(
            "wl-clipboard 2.3.0\nCopyright (C) 2018-2023 Sergey Bugaev\n"
        ));
        assert!(!is_wl_clipboard("wl-copy 1.0.1\n"));
        assert!(!is_wl_clipboard(""));
    }
}
