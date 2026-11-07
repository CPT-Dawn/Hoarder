//! What packages ship besides the binary: shell completions
//! (`hoarder completions`) and the files `hoarder dist DIR` lays out for a
//! package (man pages, completions, the systemd user unit, the default
//! config). None of it needs a Wayland session, so these run the binary
//! directly with a private HOME.

use std::path::Path;
use std::process::{Command, Output, Stdio};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

fn hoarder(home: &Path, args: &[&str]) -> Output {
    Command::new(HOARDER)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .output()
        .expect("hoarder should run")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

/// Run `script` in bash and return stdout; `$1` is the completion script.
fn bash(script: &str, completion: &Path) -> String {
    let out = Command::new("bash")
        .args(["--norc", "--noprofile", "-c", script, "bash"])
        .arg(completion)
        .output()
        .expect("bash should run");
    assert!(
        out.status.success(),
        "bash failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// What bash offers after `words` (the last word is the one being completed).
fn bash_completes(completion: &Path, words: &[&str]) -> Vec<String> {
    let script = format!(
        "source \"$1\"; COMP_WORDS=({words}); COMP_CWORD={cword}; \
         COMP_LINE=\"${{COMP_WORDS[*]}}\"; COMP_POINT=${{#COMP_LINE}}; \
         _hoarder hoarder \"${{COMP_WORDS[COMP_CWORD]}}\" \"${{COMP_WORDS[COMP_CWORD-1]}}\"; \
         printf '%s\\n' \"${{COMPREPLY[@]}}\"",
        words = words
            .iter()
            .map(|w| format!("'{w}'"))
            .collect::<Vec<_>>()
            .join(" "),
        cword = words.len() - 1,
    );
    bash(&script, completion)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn bash_completion_offers_subcommands_and_their_values() {
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("hoarder.bash");
    std::fs::write(&script, ok(&hoarder(home.path(), &["completions", "bash"]))).unwrap();

    let top = bash_completes(&script, &["hoarder", "sh"]);
    assert_eq!(top, ["shot"]);
    let targets = bash_completes(&script, &["hoarder", "shot", "w"]);
    assert_eq!(targets, ["window"]);
    // Hidden helper subcommands stay out of the user's way.
    let all = bash_completes(&script, &["hoarder", ""]);
    assert!(all.contains(&"picker".to_owned()), "{all:?}");
    assert!(!all.contains(&"serve-offer".to_owned()), "{all:?}");
    assert!(!all.contains(&"notify-capture".to_owned()), "{all:?}");
}

#[test]
fn zsh_and_fish_completions_are_scripts_for_hoarder() {
    let home = tempfile::tempdir().unwrap();
    let zsh = home.path().join("_hoarder");
    let text = ok(&hoarder(home.path(), &["completions", "zsh"]));
    assert!(text.starts_with("#compdef hoarder"), "{text}");
    std::fs::write(&zsh, text).unwrap();
    let check = Command::new("zsh").arg("-n").arg(&zsh).output().unwrap();
    assert!(
        check.status.success(),
        "zsh -n: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let fish = ok(&hoarder(home.path(), &["completions", "fish"]));
    assert!(fish.contains("complete -c hoarder"), "{fish}");
    assert!(fish.contains("picker"), "{fish}");
    assert!(!fish.contains("serve-offer"), "{fish}");
}

#[test]
fn completions_reject_an_unknown_shell_as_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let out = hoarder(home.path(), &["completions", "tcsh"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

/// Render a man page to plain text with man(1), as a user would read it.
fn man(page: &Path) -> String {
    let out = Command::new("man")
        .arg("-l")
        .arg(page)
        .env("MANPAGER", "cat")
        .env("MANWIDTH", "100")
        .output()
        .expect("man should run");
    assert!(
        out.status.success(),
        "man -l {}: {}",
        page.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    // Drop overstrike bold and underline (`x\bx`, `_\bx`).
    let mut plain = String::new();
    for c in String::from_utf8_lossy(&out.stdout).chars() {
        if c == '\u{8}' {
            plain.pop();
        } else {
            plain.push(c);
        }
    }
    plain
}

#[test]
fn dist_lays_out_what_a_package_ships() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("dist");
    ok(&hoarder(
        home.path(),
        &["dist", dir.to_str().unwrap(), "--bindir", "/usr/bin"],
    ));

    // Man pages: one for hoarder, one per public subcommand.
    let top = man(&dir.join("man/man1/hoarder.1"));
    assert!(top.contains("hoarder-shot(1)"), "{top}");
    let shot = man(&dir.join("man/man1/hoarder-shot.1"));
    assert!(shot.contains("--delay"), "{shot}");
    assert!(shot.contains("window"), "{shot}");
    assert!(dir.join("man/man1/hoarder-service.1").exists());
    assert!(!dir.join("man/man1/hoarder-serve-offer.1").exists());

    // Completions, identical to `hoarder completions`.
    for (shell, file) in [
        ("bash", "completions/hoarder.bash"),
        ("zsh", "completions/_hoarder"),
        ("fish", "completions/hoarder.fish"),
    ] {
        assert_eq!(
            std::fs::read_to_string(dir.join(file)).unwrap(),
            ok(&hoarder(home.path(), &["completions", shell])),
            "{file}"
        );
    }

    // The user unit runs the packaged binary, tied to the graphical session.
    let unit = std::fs::read_to_string(dir.join("systemd/hoarder.service")).unwrap();
    assert!(
        unit.lines()
            .any(|l| l == "ExecStart=\"/usr/bin/hoarder\" daemon"),
        "{unit}"
    );
    assert!(
        unit.lines()
            .any(|l| l == "WantedBy=graphical-session.target"),
        "{unit}"
    );

    // The default config, comments and all.
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hoarder-config/src/default.toml");
    assert_eq!(
        std::fs::read_to_string(dir.join("config.toml")).unwrap(),
        std::fs::read_to_string(default).unwrap()
    );
}

#[test]
fn service_enable_writes_the_unit_the_package_ships() {
    use std::os::unix::fs::PermissionsExt as _;

    // `hoarder service enable` in a private HOME, with a stand-in systemctl.
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let systemctl = bin.join("systemctl");
    std::fs::write(&systemctl, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    ok(&Command::new(HOARDER)
        .args(["service", "enable"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", path)
        .output()
        .unwrap());
    let written =
        std::fs::read_to_string(home.path().join(".config/systemd/user/hoarder.service")).unwrap();

    let dir = home.path().join("dist");
    let exe = std::fs::canonicalize(HOARDER).unwrap();
    let bindir = exe.parent().unwrap().to_str().unwrap();
    ok(&hoarder(
        home.path(),
        &["dist", dir.to_str().unwrap(), "--bindir", bindir],
    ));
    let shipped = std::fs::read_to_string(dir.join("systemd/hoarder.service")).unwrap();
    assert_eq!(written, shipped);
}
