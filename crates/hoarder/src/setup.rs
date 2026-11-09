//! `hoarder setup hyprland|sway|niri [--apply]`: the recommended keybindings,
//! and the line that hands the session's environment to the systemd user
//! manager so the Daemon's service can reach the compositor.
//!
//! `--apply` writes them to a file of Hoarder's own that the main compositor
//! config includes. The main config is never written: when it lacks the
//! include, the line to add is printed instead.
//!
//! Hyprland speaks two config languages. A config dir with `hyprland.lua` is
//! a Lua config: Hoarder's file is then the Lua module `hoarder.lua`, pulled
//! in by `require("hoarder")` (Hyprland looks modules up next to
//! `hyprland.lua`). Otherwise it is hyprlang's `hoarder.conf`, pulled in by
//! `source =`.

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

    /// The compositor's own config directory.
    fn dir(self) -> PathBuf {
        hoarder_config::config_home().join(match self {
            Wm::Hyprland => "hypr",
            Wm::Sway => "sway",
            Wm::Niri => "niri",
        })
    }
}

/// The language a compositor's config is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// The compositor's own format: hyprlang, Sway's config, niri's KDL.
    Native,
    /// Hyprland's Lua config (`hyprland.lua`).
    Lua,
}

/// What `setup` writes for one compositor, in the language its config uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub wm: Wm,
    pub lang: Lang,
}

impl Target {
    /// Read the compositor's config dir: Hyprland with a `hyprland.lua` is a
    /// Lua config, even beside a `hyprland.conf`, because that is the file
    /// Hyprland loads then (0.57 ignores `hyprland.conf` entirely).
    /// Everything else, a fresh Hyprland included, is native.
    pub fn detect(wm: Wm) -> Self {
        let lua = wm == Wm::Hyprland && wm.dir().join("hyprland.lua").is_file();
        Target {
            wm,
            lang: if lua { Lang::Lua } else { Lang::Native },
        }
    }

    /// The included file's contents. Deterministic, so `--apply` is
    /// idempotent.
    pub fn snippet(self) -> String {
        let header = format!(
            "Written by `hoarder setup {} --apply`; re-run it to update this file.",
            self.wm.name()
        );
        let startup = self.wm.startup_command();
        match (self.wm, self.lang) {
            (Wm::Hyprland, Lang::Lua) => {
                let bind = |keys: &str, cmd: &str| {
                    format!("hl.bind(\"{keys}\", function() hl.exec_cmd(\"{cmd}\") end)\n")
                };
                format!(
                    "-- {header}\n\
                     -- hyprland.lua loads it with `require(\"hoarder\")`.\n\
                     hl.on(\"hyprland.start\", function()\n    \
                         hl.exec_cmd(\"{startup}\")\n\
                     end)\n\
                     {}{}{}{}",
                    bind("Print", "hoarder shot"),
                    bind("SHIFT + Print", "hoarder shot all"),
                    bind("ALT + Print", "hoarder pick"),
                    bind("SUPER + V", "hoarder picker"),
                )
            }
            (Wm::Hyprland, Lang::Native) => format!(
                "# {header}\n\
                 exec-once = {startup}\n\
                 bind = , Print, exec, hoarder shot\n\
                 bind = SHIFT, Print, exec, hoarder shot all\n\
                 bind = ALT, Print, exec, hoarder pick\n\
                 bind = SUPER, V, exec, hoarder picker\n"
            ),
            (Wm::Sway, _) => format!(
                "# {header}\n\
                 exec {startup}\n\
                 bindsym Print exec hoarder shot\n\
                 bindsym Shift+Print exec hoarder shot all\n\
                 bindsym Mod1+Print exec hoarder pick\n\
                 bindsym Mod4+v exec hoarder picker\n"
            ),
            (Wm::Niri, _) => format!(
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

    /// The file `--apply` writes.
    pub fn include_path(self) -> PathBuf {
        self.wm.dir().join(match (self.wm, self.lang) {
            (Wm::Hyprland, Lang::Lua) => "hoarder.lua",
            (Wm::Hyprland, Lang::Native) => "hoarder.conf",
            (Wm::Sway, _) => "config.d/hoarder",
            (Wm::Niri, _) => "hoarder.kdl",
        })
    }

    /// The main config, which only the user edits.
    fn main_config(self) -> PathBuf {
        self.wm.dir().join(match (self.wm, self.lang) {
            (Wm::Hyprland, Lang::Lua) => "hyprland.lua",
            (Wm::Hyprland, Lang::Native) => "hyprland.conf",
            (Wm::Sway, _) => "config",
            (Wm::Niri, _) => "config.kdl",
        })
    }

    /// The line the main config needs to pull the included file in.
    fn include_line(self) -> String {
        let path = self.include_path();
        match (self.wm, self.lang) {
            (Wm::Hyprland, Lang::Lua) => "require(\"hoarder\")".into(),
            (Wm::Hyprland, Lang::Native) => format!("source = {}", path.display()),
            (Wm::Sway, _) => format!("include {}", path.display()),
            (Wm::Niri, _) => format!("include \"{}\"", path.display()),
        }
    }

    /// Whether `main` (the main config's text) already includes the file,
    /// for Sway also through a `config.d/*` glob.
    fn includes(self, main: &str) -> bool {
        main.lines()
            .map(str::trim)
            .filter(|l| !l.starts_with('#') && !l.starts_with("//") && !l.starts_with("--"))
            .any(|l| match (self.wm, self.lang) {
                (Wm::Hyprland, Lang::Lua) => requires_hoarder(l),
                (Wm::Hyprland, Lang::Native) => {
                    l.starts_with("source") && l.contains("hoarder.conf")
                }
                (Wm::Sway, _) => l.strip_prefix("include").is_some_and(|path| {
                    let path = path.trim().trim_matches(|c| c == '"' || c == '\'');
                    // The file lands in the user's config.d; the stock
                    // `include /etc/sway/config.d/*` never loads it.
                    let system = path.starts_with("/etc/") || path.starts_with("/usr/");
                    path.contains("hoarder") || (path.ends_with("config.d/*") && !system)
                }),
                (Wm::Niri, _) => l.starts_with("include") && l.contains("hoarder.kdl"),
            })
    }
}

/// Whether a Lua line calls `require` on the `hoarder` module, in any of
/// Lua's call spellings: `require("hoarder")`, `require "hoarder"`,
/// `require 'hoarder'`…
fn requires_hoarder(line: &str) -> bool {
    let code = line.split("--").next().unwrap_or_default();
    let squeezed: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '(' && *c != ')')
        .collect();
    squeezed.contains("require\"hoarder\"") || squeezed.contains("require'hoarder'")
}

pub fn run(wm: Wm, apply: bool, yes: bool) -> Result<(), Fail> {
    let target = Target::detect(wm);
    let snippet = target.snippet();
    let comment = if target.lang == Lang::Lua { "--" } else { "#" };
    if !apply {
        print!("{snippet}");
        println!(
            "\n{comment} `hoarder setup {} --apply` writes these to {}",
            wm.name(),
            target.include_path().display()
        );
        return Ok(());
    }
    let path = target.include_path();
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
    let main = target.main_config();
    let included = std::fs::read_to_string(&main).is_ok_and(|text| target.includes(&text));
    if !included {
        println!(
            "Add this line to {} (Hoarder never edits it):\n    {}",
            main.display(),
            target.include_line()
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
    fn sways_stock_system_include_does_not_count_as_hoarders() {
        let sway = native(Wm::Sway);
        // The last line of /etc/sway/config: it never loads ~/.config/sway/config.d.
        assert!(!sway.includes("include /etc/sway/config.d/*\n"));
        assert!(!sway.includes("include '/etc/sway/config.d/*'\n"));
        assert!(sway.includes("include ~/.config/sway/config.d/*\n"));
        assert!(sway.includes("include $HOME/.config/sway/config.d/*\n"));
        assert!(sway.includes("include config.d/*\n"));
        assert!(sway.includes("include ~/.config/sway/config.d/hoarder\n"));
    }

    #[test]
    fn diff_marks_kept_removed_and_added_lines() {
        assert_eq!(diff("a\nb\nc\n", "a\nx\nc\nd\n"), " a\n-b\n+x\n c\n+d\n");
    }

    fn native(wm: Wm) -> Target {
        Target {
            wm,
            lang: Lang::Native,
        }
    }

    const LUA: Target = Target {
        wm: Wm::Hyprland,
        lang: Lang::Lua,
    };

    #[test]
    fn includes_ignores_commented_out_lines() {
        let hyprlang = native(Wm::Hyprland);
        assert!(!hyprlang.includes("# source = ~/.config/hypr/hoarder.conf\n"));
        assert!(hyprlang.includes("source = ~/.config/hypr/hoarder.conf\n"));
        assert!(native(Wm::Niri).includes("include \"hoarder.kdl\"\n"));
        assert!(!native(Wm::Niri).includes("// include \"hoarder.kdl\"\n"));
    }

    #[test]
    fn lua_include_is_a_require_of_the_hoarder_module() {
        for spelling in [
            "require(\"hoarder\")",
            "require \"hoarder\"",
            "require 'hoarder'",
            "  require( 'hoarder' ) -- clipboard",
            "local _ = require(\"hoarder\")",
        ] {
            assert!(LUA.includes(spelling), "{spelling}");
        }
        for other in [
            "-- require(\"hoarder\")",
            "require(\"modules.hoarder\")",
            "require(\"hoarderx\")",
            "print(\"hoarder\") -- require(\"hoarder\")",
            "source = ~/.config/hypr/hoarder.conf",
        ] {
            assert!(!LUA.includes(other), "{other}");
        }
        assert!(!native(Wm::Hyprland).includes("require(\"hoarder\")"));
    }

    #[test]
    fn lua_snippet_uses_hyprlands_lua_api() {
        let lua = LUA.snippet();
        assert!(lua.starts_with("-- Written by `hoarder setup hyprland --apply`"));
        assert!(lua.contains(
            "hl.bind(\"SHIFT + Print\", function() hl.exec_cmd(\"hoarder shot all\") end)\n"
        ));
        assert!(lua.contains(
            "hl.on(\"hyprland.start\", function()\n    hl.exec_cmd(\"systemctl --user \
             import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP HYPRLAND_INSTANCE_SIGNATURE \
             && systemctl --user start hoarder.service\")\nend)\n"
        ));
        assert_eq!(LUA.include_line(), "require(\"hoarder\")");
        assert!(LUA.include_path().ends_with("hypr/hoarder.lua"));
        assert!(LUA.main_config().ends_with("hypr/hyprland.lua"));
    }
}
