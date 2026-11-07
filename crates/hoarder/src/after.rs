//! Everything that follows a delivered Capture: hook, notification, sound
//! (PLAN.md §4.1 step 7). None of it can fail the Capture; problems are logged.

use std::ffi::OsStr;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hoarder_config::Screenshot;

use crate::cut::Badges;

/// A Capture that has reached the clipboard and/or disk.
#[derive(Debug)]
pub struct Delivered {
    /// The file it was written to, if any (not for `-o -` or `save = false`).
    pub path: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// `all`, `region`, `window` or `output`.
    pub target: &'static str,
    pub badges: Badges,
}

/// Follow up the images of one Capture: one, or one per output when split.
/// The shutter sounds once; each image gets its hook run and notification.
pub fn run(captures: &[Delivered], settings: &Screenshot) {
    if settings.sound {
        play_shutter();
    }
    for capture in captures {
        crate::hook::spawn(&settings.on_capture, capture);
        if settings.notify {
            crate::notify::spawn(capture);
        }
    }
}

/// Start the flash, if enabled, alongside the rest of the shot. It runs after
/// the freeze, so it never shows in the Capture; dropping the guard waits for
/// it to fade, as its surfaces go with the process.
pub fn flash(settings: &Screenshot) -> Option<Flash> {
    settings
        .flash
        .then(|| Flash(Some(std::thread::spawn(hoarder_wayland::flash::flash))))
}

#[derive(Debug)]
pub struct Flash(Option<std::thread::JoinHandle<Result<(), String>>>);

impl Drop for Flash {
    fn drop(&mut self) {
        if let Some(Ok(Err(e))) = self.0.take().map(std::thread::JoinHandle::join) {
            eprintln!("hoarder: no flash: {e}");
        }
    }
}

/// The freedesktop `screen-capture` sound, through libcanberra if it is
/// installed, else straight from the sound theme with PipeWire or PulseAudio.
fn play_shutter() {
    const SOUND: &str = "/usr/share/sounds/freedesktop/stereo/screen-capture.oga";
    if on_path("canberra-gtk-play") {
        let args = ["-i", "screen-capture", "-d", "Hoarder"].map(OsStr::new);
        run_detached("canberra-gtk-play", &args);
    } else if Path::new(SOUND).is_file() {
        let player = ["pw-play", "paplay"].into_iter().find(|p| on_path(p));
        match player {
            Some(player) => run_detached(player, &[OsStr::new(SOUND)]),
            None => {
                eprintln!("hoarder: no sound player found (canberra-gtk-play, pw-play, paplay)")
            }
        }
    }
}

pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// Start `program` with no stdio of ours and never wait for it.
pub fn run_detached(program: &str, args: &[&OsStr]) {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("hoarder: cannot run {program}: {e}"),
    }
}
