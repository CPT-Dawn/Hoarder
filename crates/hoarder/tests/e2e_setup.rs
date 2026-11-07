//! `hoarder setup hyprland|sway|niri [--apply]`: recommended keybindings,
//! written only to an included file of Hoarder's own. Every run happens in a
//! session's private HOME, so the real compositor config is never in reach.

use std::path::PathBuf;
use std::process::{Output, Stdio};

use hoarder_e2e::Session;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(HOARDER)
        .args(args)
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

fn config_home(session: &Session) -> PathBuf {
    session.runtime_dir().join("home/.config")
}

#[test]
fn setup_sway_prints_bindings_and_writes_nothing() {
    let session = Session::start_default().unwrap();

    let out = ok(&hoarder(&session, &["setup", "sway"]));

    assert!(out.contains("bindsym Print exec hoarder shot"), "{out}");
    assert!(out.contains("bindsym Mod4+v exec hoarder picker"), "{out}");
    // The service needs the session's WAYLAND_DISPLAY in the user manager.
    assert!(
        out.contains("systemctl --user import-environment WAYLAND_DISPLAY"),
        "{out}"
    );
    assert!(!config_home(&session).join("sway").exists());
}

#[test]
fn setup_apply_writes_only_the_included_file_and_is_idempotent() {
    let session = Session::start_default().unwrap();
    let main = config_home(&session).join("sway/config");
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    let main_text = "set $mod Mod4\nbindsym $mod+Return exec foot\n";
    std::fs::write(&main, main_text).unwrap();
    let included = config_home(&session).join("sway/config.d/hoarder");

    let first = ok(&hoarder(&session, &["setup", "sway", "--apply"]));
    let written = std::fs::read(&included).unwrap();
    let second = ok(&hoarder(&session, &["setup", "sway", "--apply"]));

    assert!(
        String::from_utf8_lossy(&written).contains("bindsym Print exec hoarder shot"),
        "{}",
        String::from_utf8_lossy(&written)
    );
    assert_eq!(
        std::fs::read(&included).unwrap(),
        written,
        "second run changed the file"
    );
    assert!(
        first.contains(&format!("Wrote {}", included.display())),
        "{first}"
    );
    assert!(second.contains("already up to date"), "{second}");
    // The main config is never edited; the include line is offered instead.
    assert_eq!(std::fs::read_to_string(&main).unwrap(), main_text);
    assert!(
        second.contains(&format!("include {}", included.display())),
        "{second}"
    );
}

#[test]
fn setup_apply_stays_quiet_about_an_include_that_is_already_there() {
    let session = Session::start_default().unwrap();
    let main = config_home(&session).join("sway/config");
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    std::fs::write(&main, "include ~/.config/sway/config.d/*\n").unwrap();

    let out = ok(&hoarder(&session, &["setup", "sway", "--apply"]));

    assert!(!out.contains("Add this line"), "{out}");
}

#[test]
fn setup_apply_shows_a_diff_instead_of_overwriting_an_edited_file() {
    let session = Session::start_default().unwrap();
    ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));
    let included = config_home(&session).join("hypr/hoarder.conf");
    let edited = std::fs::read_to_string(&included)
        .unwrap()
        .replace("bind = SUPER, V,", "bind = SUPER ALT, V,");
    std::fs::write(&included, &edited).unwrap();

    // Not a terminal, so nobody can be asked: refuse and show what would change.
    let refused = hoarder(&session, &["setup", "hyprland", "--apply"]);

    assert_eq!(refused.status.code(), Some(1));
    let shown = String::from_utf8_lossy(&refused.stdout);
    assert!(
        shown.contains("-bind = SUPER ALT, V, exec, hoarder picker"),
        "{shown}"
    );
    assert!(
        shown.contains("+bind = SUPER, V, exec, hoarder picker"),
        "{shown}"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--yes"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(std::fs::read_to_string(&included).unwrap(), edited);

    ok(&hoarder(
        &session,
        &["setup", "hyprland", "--apply", "--yes"],
    ));
    assert!(
        std::fs::read_to_string(&included)
            .unwrap()
            .contains("bind = SUPER, V, exec, hoarder picker")
    );
}

#[test]
fn setup_apply_never_creates_a_missing_main_config() {
    let session = Session::start_default().unwrap();

    for wm in ["hyprland", "sway", "niri"] {
        ok(&hoarder(&session, &["setup", wm, "--apply"]));
    }

    let home = config_home(&session);
    assert!(home.join("hypr/hoarder.conf").is_file());
    assert!(home.join("sway/config.d/hoarder").is_file());
    assert!(home.join("niri/hoarder.kdl").is_file());
    assert!(!home.join("hypr/hyprland.conf").exists());
    assert!(!home.join("sway/config").exists());
    assert!(!home.join("niri/config.kdl").exists());
}
