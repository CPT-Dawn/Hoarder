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
    let exe = exe.to_string_lossy();
    Ok(format!(
        "# Written by `hoarder service enable`; `hoarder service disable` removes it.\n\
         [Unit]\n\
         Description=Hoarder clipboard Daemon\n\
         Documentation=https://github.com/CPT-Dawn/Hoarder\n\
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
    ))
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

fn status() -> Result<(), (Exit, String)> {
    // `is-enabled` exits non-zero for "disabled", so read stdout either way.
    let enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", UNIT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
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
