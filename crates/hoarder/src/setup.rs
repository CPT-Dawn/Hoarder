//! `hoarder setup hyprland|sway|niri [--apply]`: the recommended keybindings,
//! and the line that hands the session's environment to the systemd user
//! manager so the Daemon's service can reach the compositor.
//!
//! `--apply` writes them to a file of Hoarder's own that the main compositor
//! config includes. The main config is never written: when it lacks the
//! include, the line to add is printed instead.

use std::path::PathBuf;

use clap::ValueEnum;

use crate::Exit;

type Fail = (Exit, String);

/// A compositor `setup` knows how to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Wm {
    Hyprland,
    Sway,
    Niri,
}

impl Wm {
    fn name(self) -> &'static str {
        match self {
            Wm::Hyprland => "hyprland",
            Wm::Sway => "sway",
            Wm::Niri => "niri",
        }
    }

    /// The variable that points clients at this compositor's IPC, which the
    /// Daemon's service needs too (window geometry, focus).
    fn ipc_var(self) -> &'static str {
        match self {
            Wm::Hyprland => "HYPRLAND_INSTANCE_SIGNATURE",
            Wm::Sway => "SWAYSOCK",
            Wm::Niri => "NIRI_SOCKET",
        }
    }

    /// The shell command run once at login: export the session to the user
    /// manager, then start the service (plain compositor sessions never
    /// reach `graphical-session.target`, which the unit is wanted by).
    fn startup_command(self) -> String {
        format!(
            "systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP {} \
             && systemctl --user start hoarder.service",
            self.ipc_var()
        )
    }

    /// The included file's contents. Deterministic, so `--apply` is
    /// idempotent.
    pub fn snippet(self) -> String {
        let header = format!(
            "Written by `hoarder setup {} --apply`; re-run it to update this file.",
            self.name()
        );
        let startup = self.startup_command();
        match self {
            Wm::Hyprland => format!(
                "# {header}\n\
                 exec-once = {startup}\n\
                 bind = , Print, exec, hoarder shot\n\
                 bind = SHIFT, Print, exec, hoarder shot all\n\
                 bind = ALT, Print, exec, hoarder pick\n\
                 bind = SUPER, V, exec, hoarder picker\n"
            ),
            Wm::Sway => format!(
                "# {header}\n\
                 exec {startup}\n\
                 bindsym Print exec hoarder shot\n\
                 bindsym Shift+Print exec hoarder shot all\n\
                 bindsym Mod1+Print exec hoarder pick\n\
                 bindsym Mod4+v exec hoarder picker\n"
            ),
            Wm::Niri => format!(
                "// {header}\n\
                 spawn-at-startup \"sh\" \"-c\" \"{startup}\"\n\
                 binds {{\n    \
                     Print {{ spawn \"hoarder\" \"shot\"; }}\n    \
                     Shift+Print {{ spawn \"hoarder\" \"shot\" \"all\"; }}\n    \
                     Alt+Print {{ spawn \"hoarder\" \"pick\"; }}\n    \
                     Mod+V {{ spawn \"hoarder\" \"picker\"; }}\n\
                 }}\n"
            ),
        }
    }

    /// The compositor's own config directory.
    fn dir(self) -> PathBuf {
        hoarder_config::config_home().join(match self {
            Wm::Hyprland => "hypr",
            Wm::Sway => "sway",
            Wm::Niri => "niri",
        })
    }

    /// The file `--apply` writes.
    pub fn include_path(self) -> PathBuf {
        match self {
            Wm::Hyprland => self.dir().join("hoarder.conf"),
            Wm::Sway => self.dir().join("config.d/hoarder"),
            Wm::Niri => self.dir().join("hoarder.kdl"),
        }
    }

    /// The main config, which only the user edits.
    fn main_config(self) -> PathBuf {
        self.dir().join(match self {
            Wm::Hyprland => "hyprland.conf",
            Wm::Sway => "config",
            Wm::Niri => "config.kdl",
        })
    }

    /// The line the main config needs to pull the included file in.
    fn include_line(self) -> String {
        let path = self.include_path();
        match self {
            Wm::Hyprland => format!("source = {}", path.display()),
            Wm::Sway => format!("include {}", path.display()),
            Wm::Niri => format!("include \"{}\"", path.display()),
        }
    }

    /// Whether `main` (the main config's text) already includes the file,
    /// for Sway also through a `config.d/*` glob.
    fn includes(self, main: &str) -> bool {
        main.lines()
            .map(str::trim)
            .filter(|l| !l.starts_with('#') && !l.starts_with("//"))
            .any(|l| match self {
                Wm::Hyprland => l.starts_with("source") && l.contains("hoarder.conf"),
                Wm::Sway => {
                    l.starts_with("include") && (l.contains("hoarder") || l.contains("config.d/*"))
                }
                Wm::Niri => l.starts_with("include") && l.contains("hoarder.kdl"),
            })
    }
}

pub fn run(wm: Wm, apply: bool, yes: bool) -> Result<(), Fail> {
    let snippet = wm.snippet();
    if !apply {
        print!("{snippet}");
        println!(
            "\n# `hoarder setup {} --apply` writes these to {}",
            wm.name(),
            wm.include_path().display()
        );
        return Ok(());
    }
    let path = wm.include_path();
    let io = |e: std::io::Error| (Exit::Io, format!("cannot write {}: {e}", path.display()));
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err((Exit::Io, format!("cannot read {}: {e}", path.display()))),
    };
    match current {
        Some(current) if current == snippet => {
            println!("{} is already up to date.", path.display());
        }
        Some(current) if !yes && !confirm(&path, &current, &snippet)? => {
            return Err((
                Exit::Cancelled,
                format!(
                    "left {} as it is; re-run with --yes to replace it",
                    path.display()
                ),
            ));
        }
        _ => {
            std::fs::create_dir_all(path.parent().expect("include path has a directory"))
                .map_err(io)?;
            std::fs::write(&path, &snippet).map_err(io)?;
            println!("Wrote {}.", path.display());
        }
    }
    let main = wm.main_config();
    let included = std::fs::read_to_string(&main).is_ok_and(|text| wm.includes(&text));
    if !included {
        println!(
            "Add this line to {} (Hoarder never edits it):\n    {}",
            main.display(),
            wm.include_line()
        );
    }
    Ok(())
}

/// Show how `path` would change and, on a terminal, ask whether to go
/// ahead. Without a terminal there is nobody to ask: the answer is no.
fn confirm(path: &std::path::Path, old: &str, new: &str) -> Result<bool, Fail> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};

    println!("{} differs from the recommended bindings:", path.display());
    println!("--- {}\n+++ recommended", path.display());
    print!("{}", diff(old, new));
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("Replace it? [y/N] ");
    std::io::stdout()
        .flush()
        .map_err(|e| (Exit::Io, e.to_string()))?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| (Exit::Io, e.to_string()))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// A whole-file line diff: ` ` kept, `-` removed, `+` added (longest common
/// subsequence; the files are a handful of lines).
fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // lcs[i][j]: common lines of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out += &format!(" {}\n", a[i]);
            (i, j) = (i + 1, j + 1);
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out += &format!("-{}\n", a[i]);
            i += 1;
        } else {
            out += &format!("+{}\n", b[j]);
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_kept_removed_and_added_lines() {
        assert_eq!(diff("a\nb\nc\n", "a\nx\nc\nd\n"), " a\n-b\n+x\n c\n+d\n");
    }

    #[test]
    fn includes_ignores_commented_out_lines() {
        assert!(!Wm::Hyprland.includes("# source = ~/.config/hypr/hoarder.conf\n"));
        assert!(Wm::Hyprland.includes("source = ~/.config/hypr/hoarder.conf\n"));
        assert!(Wm::Niri.includes("include \"hoarder.kdl\"\n"));
        assert!(!Wm::Niri.includes("// include \"hoarder.kdl\"\n"));
    }
}
