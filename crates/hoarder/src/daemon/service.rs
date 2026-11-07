//! `hoarder service enable|disable|status`: the Daemon as a systemd user
//! service bound to the graphical session (ADR-0001).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use clap::Subcommand;

use crate::Exit;

const UNIT: &str = "hoarder.service";

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Install the user unit and start the Daemon now and on every login.
    Enable,
    /// Stop the Daemon and remove the user unit.
    Disable,
    /// Whether the service is enabled and the Daemon answers (exit 1 if not).
    Status,
}

pub fn run(command: ServiceCommand) -> Result<(), (Exit, String)> {
    match command {
        ServiceCommand::Enable => enable(),
        ServiceCommand::Disable => disable(),
        ServiceCommand::Status => status(),
    }
}

/// `$XDG_CONFIG_HOME/systemd/user/hoarder.service`.
fn unit_path() -> Result<PathBuf, (Exit, String)> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| {
            (
                Exit::Io,
                "neither XDG_CONFIG_HOME nor HOME is set".to_owned(),
            )
        })?;
    Ok(config_home.join("systemd/user").join(UNIT))
}

fn unit_text() -> Result<String, (Exit, String)> {
    let exe = std::env::current_exe()
        .map_err(|e| (Exit::Io, format!("cannot find the hoarder binary: {e}")))?;
    Ok(unit_for(&exe))
}

/// The user unit that runs `exe daemon`: what `hoarder service enable`
/// writes, and (for the packaged binary) what packages ship.
pub fn unit_for(exe: &std::path::Path) -> String {
    let exe = exe.to_string_lossy();
    format!(
        "# The Hoarder Daemon; `hoarder service enable|disable` turns it on and off.\n\
         [Unit]\n\
         Description=Hoarder clipboard Daemon\n\
         Documentation=https://github.com/CPT-Dawn/Hoarder man:hoarder(1)\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart=\"{exe}\" daemon\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    )
}

/// Run `systemctl --user ARGS`; a failure carries systemctl's own words.
fn systemctl(args: &[&str]) -> Result<String, (Exit, String)> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| (Exit::Io, format!("cannot run systemctl: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err((
            Exit::Io,
            format!(
                "systemctl --user {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ))
    }
}

fn enable() -> Result<(), (Exit, String)> {
    let path = unit_path()?;
    let io = |e: std::io::Error| (Exit::Io, format!("cannot write {}: {e}", path.display()));
    std::fs::create_dir_all(path.parent().expect("unit path has a directory")).map_err(io)?;
    std::fs::write(&path, unit_text()?).map_err(io)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", UNIT])?;
    println!("Enabled {UNIT}: the Daemon runs now and on every login.");
    Ok(())
}

fn disable() -> Result<(), (Exit, String)> {
    systemctl(&["disable", "--now", UNIT])?;
    let path = unit_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err((Exit::Io, format!("cannot remove {}: {e}", path.display()))),
    }
    systemctl(&["daemon-reload"])?;
    println!("Disabled {UNIT}: copies now last only as long as their app.");
    Ok(())
}

/// `systemctl --user is-enabled` for the unit ("enabled", "disabled", …),
/// "not installed" when systemd does not know it, `None` without systemctl.
pub fn enabled_state() -> Option<String> {
    // `is-enabled` exits non-zero for "disabled", so read stdout either way.
    let out = Command::new("systemctl")
        .args(["--user", "is-enabled", UNIT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let state = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Some(if state.is_empty() {
        "not installed".into()
    } else {
        state
    })
}

/// The user manager's `WAYLAND_DISPLAY`, which the service inherits:
/// `Err` when the manager cannot be asked, `Ok(None)` when it is unset.
pub fn manager_wayland_display() -> Result<Option<String>, (Exit, String)> {
    let env = systemctl(&["show-environment"])?;
    Ok(env
        .lines()
        .find_map(|l| l.strip_prefix("WAYLAND_DISPLAY="))
        .map(str::to_owned))
}

fn status() -> Result<(), (Exit, String)> {
    let enabled = enabled_state().unwrap_or_else(|| "unknown".into());
    println!("service: {enabled}");
    let status = hoarder_ipc::Connection::connect()
        .ok()
        .and_then(|mut c| c.status().ok())
        .and_then(Result::ok);
    match status {
        Some(s) => {
            println!(
                "daemon: running (pid {}, version {}, {}-data-control)",
                s.pid, s.daemon, s.data_control
            );
            Ok(())
        }
        None => Err((
            Exit::Cancelled,
            "the Daemon is not running; start it with `hoarder service enable`".into(),
        )),
    }
}
