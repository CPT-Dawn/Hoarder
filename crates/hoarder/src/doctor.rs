//! `hoarder doctor`: the compositor, the protocols it offers, the paths
//! Hoarder chose, the service's state and any conflicting clipboard tools.
//! Every problem comes with a fix.

use crate::Exit;

type Fail = (Exit, String);

/// Something wrong, and what to do about it. Both are required, so no
/// problem can be reported without a fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub what: String,
    pub fix: String,
}

impl Problem {
    fn new(what: impl Into<String>, fix: impl Into<String>) -> Problem {
        Problem {
            what: what.into(),
            fix: fix.into(),
        }
    }
}

/// What doctor found: labelled facts, then problems.
#[derive(Debug, Default)]
struct Report {
    facts: Vec<(&'static str, String)>,
    problems: Vec<Problem>,
}

impl Report {
    fn fact(&mut self, label: &'static str, value: impl Into<String>) {
        self.facts.push((label, value.into()));
    }

    fn problem(&mut self, problem: Problem) {
        self.problems.push(problem);
    }

    fn print(&self) {
        let width = self.facts.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
        for (label, value) in &self.facts {
            println!("{label:width$}  {value}");
        }
        println!();
        match self.problems.len() {
            0 => println!("No problems found."),
            1 => println!("1 problem:"),
            n => println!("{n} problems:"),
        }
        for p in &self.problems {
            println!("  ! {}\n    fix: {}", p.what, p.fix);
        }
    }
}

/// The compositor this process runs under, from the variables its clients
/// inherit.
fn compositor() -> String {
    let set = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
    if set("HYPRLAND_INSTANCE_SIGNATURE") {
        "Hyprland".into()
    } else if set("SWAYSOCK") {
        "Sway".into()
    } else if set("NIRI_SOCKET") {
        "niri".into()
    } else {
        std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }
}

const UPDATE: &str =
    "update the compositor; Hyprland, Sway, niri and COSMIC releases from 2025 on offer it";

fn wayland(report: &mut Report) {
    report.fact("compositor", compositor());
    let globals = match hoarder_wayland::probe::globals() {
        Ok(g) => g,
        Err(e) => {
            report.problem(Problem::new(
                format!("cannot connect to a Wayland compositor: {e}"),
                "run Hoarder inside your Wayland session, with WAYLAND_DISPLAY set",
            ));
            return;
        }
    };

    let capture = if globals.has("ext_image_copy_capture_manager_v1")
        && globals.has("ext_output_image_capture_source_manager_v1")
    {
        "ext-image-copy-capture-v1"
    } else if globals.has("zwlr_screencopy_manager_v1") {
        "wlr-screencopy-unstable-v1"
    } else {
        report.problem(Problem::new(
            "the compositor offers no screen capture protocol, so `hoarder shot` and `pick` cannot work",
            "use a compositor with ext-image-copy-capture-v1 or wlr-screencopy (Hyprland, Sway, niri, COSMIC); \
             KWin and GNOME only capture through the portal",
        ));
        "none"
    };
    report.fact("capture", capture);

    let clipboard = if globals.has("ext_data_control_manager_v1") {
        "ext-data-control-v1"
    } else if globals.has("zwlr_data_control_manager_v1") {
        report.problem(Problem::new(
            "only wlr-data-control is offered: the Daemon works, but `hoarder copy` and `paste` need ext-data-control-v1",
            UPDATE,
        ));
        "wlr-data-control-unstable-v1"
    } else {
        report.problem(Problem::new(
            "the compositor offers no data-control protocol, so neither the Daemon nor `hoarder copy`/`paste` can work",
            UPDATE,
        ));
        "none"
    };
    report.fact("clipboard", clipboard);

    let overlay = if !globals.has("zwlr_layer_shell_v1") {
        report.problem(Problem::new(
            "the compositor offers no wlr-layer-shell, so the Overlay cannot be shown",
            "use a compositor with wlr-layer-shell (Hyprland, Sway, niri, COSMIC)",
        ));
        "none"
    } else if !globals.has("wp_viewporter") {
        report.problem(Problem::new(
            "the compositor offers no wp_viewporter, so the Overlay cannot be shown",
            UPDATE,
        ));
        "none"
    } else {
        "wlr-layer-shell-v1"
    };
    report.fact("overlay", overlay);

    let windows = hoarder_wayland::windows::detect();
    let name = windows.name();
    if name == "none" {
        report.fact(
            "windows",
            "none (no window snapping; Region and Output capture still work)",
        );
    } else {
        report.fact("windows", name);
        if let Err(e) = windows.windows() {
            report.problem(Problem::new(
                format!("{name} did not answer: {e}"),
                "check that HYPRLAND_INSTANCE_SIGNATURE or SWAYSOCK points at the running compositor",
            ));
        }
    }
}

fn paths(report: &mut Report) {
    let path = hoarder_config::path();
    report.fact("config", path.display().to_string());
    let config = match hoarder_config::load(&path) {
        Ok(config) => config,
        Err(e) => {
            report.problem(Problem::new(
                format!("{e}; the defaults are used instead"),
                "run `hoarder config check` to see every problem, then `hoarder config edit`",
            ));
            hoarder_config::Config::default()
        }
    };
    match hoarder_config::expand_dir(&config.screenshot.dir) {
        Ok(dir) => report.fact("screenshots", dir.display().to_string()),
        Err(e) => report.problem(Problem::new(
            format!("screenshot.dir {:?}: {e}", config.screenshot.dir),
            "`hoarder config set screenshot.dir ~/Pictures/Screenshots`",
        )),
    }
    if let Some(socket) = hoarder_ipc::socket_path() {
        report.fact("socket", socket.display().to_string());
    } else {
        report.problem(Problem::new(
            "XDG_RUNTIME_DIR is not set, so the Daemon has nowhere to listen",
            "log in through a session manager (systemd-logind or elogind) that sets XDG_RUNTIME_DIR",
        ));
    }
}

/// How to get the session's environment into the user manager on this
/// compositor.
fn export_fix() -> String {
    match compositor().as_str() {
        "Hyprland" | "Sway" | "niri" => format!(
            "`hoarder setup {} --apply` adds the export to your compositor's autostart",
            compositor().to_lowercase()
        ),
        _ => "run `systemctl --user import-environment WAYLAND_DISPLAY` from your compositor's autostart".into(),
    }
}

fn service(report: &mut Report) {
    use crate::daemon::service;

    let state = service::enabled_state();
    report.fact(
        "service",
        state.as_deref().unwrap_or("unknown (no systemctl)"),
    );
    let status = hoarder_ipc::Connection::connect()
        .ok()
        .and_then(|mut c| c.status().ok())
        .and_then(Result::ok);
    match &status {
        Some(s) => report.fact(
            "daemon",
            format!(
                "running (pid {}, version {}, {}-data-control)",
                s.pid, s.daemon, s.data_control
            ),
        ),
        None => report.fact("daemon", "not running"),
    }

    let enabled = state.as_deref() == Some("enabled");
    match (enabled, status.is_some(), state.is_some()) {
        (_, true, _) => {}
        (true, false, _) => report.problem(Problem::new(
            "the service is enabled but the Daemon does not answer",
            "`systemctl --user restart hoarder.service`; `journalctl --user -u hoarder.service` says why it stopped",
        )),
        (false, false, true) => report.problem(Problem::new(
            "the Daemon is not running: copies vanish with the app they came from, and History is off",
            "`hoarder service enable` starts it now and on every login",
        )),
        (false, false, false) => report.problem(Problem::new(
            "the Daemon is not running and systemd is not available to run it",
            "start `hoarder daemon` from your compositor's autostart",
        )),
    }

    if enabled {
        let ours = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        match service::manager_wayland_display() {
            Ok(Some(theirs)) if theirs == ours => {}
            Ok(Some(theirs)) => report.problem(Problem::new(
                format!(
                    "the systemd user manager has WAYLAND_DISPLAY={theirs}, not this session's {ours}, \
                     so the service talks to the wrong display"
                ),
                export_fix(),
            )),
            Ok(None) => report.problem(Problem::new(
                "the systemd user manager has no WAYLAND_DISPLAY, so the service cannot reach the compositor",
                export_fix(),
            )),
            Err((_, e)) => report.problem(Problem::new(
                format!("cannot read the user manager's environment: {e}"),
                "check that `systemctl --user show-environment` works in this session",
            )),
        }
    }
}

/// The clipboard tool a command line belongs to, if it fights Hoarder.
fn conflict(pid: u32, argv: &[String]) -> Option<Problem> {
    let program = argv.first()?.rsplit('/').next()?;
    let line = argv.join(" ");
    match program {
        "wl-clip-persist" => Some(Problem::new(
            format!(
                "wl-clip-persist (pid {pid}) re-offers every copy at once, which breaks pasting \
                 in Firefox, GTK apps, IntelliJ, Inkscape and GIMP; the Daemon keeps copies without that"
            ),
            "stop it (`kill {pid}`) and remove it from your compositor's autostart",
        )),
        "wl-paste" => {
            // `-w`/`--watch` takes the rest of the line as the command.
            let watch = argv[1..].iter().position(|a| {
                a == "--watch" || (a.starts_with('-') && !a.starts_with("--") && a.contains('w'))
            })?;
            let command = &argv[watch + 2..];
            if command.first().is_some_and(|c| c.ends_with("cliphist")) {
                Some(Problem::new(
                    format!(
                        "cliphist (pid {pid}, `{line}`) keeps a second History on disk, \
                         secrets included"
                    ),
                    format!(
                        "stop it (`kill {pid}`) and remove that line from your autostart; \
                         `hoarder history` and the Picker replace it"
                    ),
                ))
            } else {
                Some(Problem::new(
                    format!(
                        "a wl-paste watcher (pid {pid}, `{line}`) starts a process for every copy"
                    ),
                    format!(
                        "stop it (`kill {pid}`) and remove it from your autostart; to run a command \
                         for each copy, set `clipboard.on_clip` (`hoarder config set clipboard.on_clip …`)"
                    ),
                ))
            }
        }
        _ => None,
    }
}

/// This user's processes in this Wayland session (same `XDG_RUNTIME_DIR`
/// and `WAYLAND_DISPLAY`): `(pid, argv)`.
fn session_processes() -> Vec<(u32, Vec<String>)> {
    use std::os::unix::fs::MetadataExt as _;

    let wanted = |key: &str| std::env::var(key).unwrap_or_default();
    let (runtime, display) = (wanted("XDG_RUNTIME_DIR"), wanted("WAYLAND_DISPLAY"));
    let uid = rustix::process::getuid().as_raw();
    let me = std::process::id();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let split = |bytes: Vec<u8>| -> Vec<String> {
        bytes
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let dir = entry.path();
            if pid == me || dir.metadata().ok()?.uid() != uid {
                return None;
            }
            let environ = split(std::fs::read(dir.join("environ")).ok()?);
            let has =
                |key: &str, value: &str| environ.iter().any(|e| *e == format!("{key}={value}"));
            if !has("XDG_RUNTIME_DIR", &runtime) || !has("WAYLAND_DISPLAY", &display) {
                return None;
            }
            Some((pid, split(std::fs::read(dir.join("cmdline")).ok()?)))
        })
        .collect()
}

fn conflicts(report: &mut Report) {
    let mut found: Vec<(u32, Problem)> = session_processes()
        .into_iter()
        .filter_map(|(pid, argv)| Some((pid, conflict(pid, &argv)?)))
        .collect();
    found.sort_by_key(|(pid, _)| *pid);
    report.fact(
        "conflicts",
        if found.is_empty() {
            "none".to_owned()
        } else {
            found.len().to_string()
        },
    );
    for (_, problem) in found {
        report.problem(problem);
    }
}

pub fn run() -> Result<(), Fail> {
    let mut report = Report::default();
    wayland(&mut report);
    paths(&mut report);
    service(&mut report);
    conflicts(&mut report);
    report.print();
    match report.problems.len() {
        0 => Ok(()),
        n => Err((
            Exit::Cancelled,
            format!("{n} problem{} found", if n == 1 { "" } else { "s" }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn a_one_shot_wl_paste_is_no_conflict() {
        assert_eq!(conflict(7, &argv("wl-paste -n")), None);
        assert_eq!(
            conflict(7, &argv("/usr/bin/wl-paste --type text/plain")),
            None
        );
    }

    #[test]
    fn watchers_are_found_however_the_flag_is_spelled() {
        for line in [
            "wl-paste -w notify-send copied",
            "/usr/bin/wl-paste -nw notify-send copied",
            "wl-paste --type text --watch notify-send copied",
        ] {
            let p = conflict(7, &argv(line)).unwrap_or_else(|| panic!("{line}"));
            assert!(p.fix.contains("clipboard.on_clip"), "{line}");
        }
    }

    #[test]
    fn cliphist_behind_a_watcher_is_named() {
        let p = conflict(
            7,
            &argv("wl-paste --type image --watch /usr/bin/cliphist store"),
        )
        .unwrap();
        assert!(p.what.starts_with("cliphist (pid 7"), "{}", p.what);
    }

    #[test]
    fn unrelated_programs_are_no_conflict() {
        assert_eq!(conflict(7, &argv("kitty")), None);
        assert_eq!(conflict(7, &[]), None);
    }
}
