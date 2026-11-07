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

// ---- Hyprland's two config languages -------------------------------------------

/// A Hyprland config dir holding `main` (`hyprland.lua` or `hyprland.conf`)
/// with `text`.
fn hypr_with(session: &Session, main: &str, text: &str) -> PathBuf {
    let dir = config_home(session).join("hypr");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(main), text).unwrap();
    dir
}

#[test]
fn setup_hyprland_writes_a_lua_module_for_a_lua_config() {
    let session = Session::start_default().unwrap();
    let main_text = "require(\"modules.binds\")\n";
    let dir = hypr_with(&session, "hyprland.lua", main_text);

    let out = ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));

    let module = std::fs::read_to_string(dir.join("hoarder.lua")).unwrap();
    assert!(
        module.contains(r#"hl.bind("Print", function() hl.exec_cmd("hoarder shot") end)"#),
        "{module}"
    );
    assert!(
        module.contains(r#"hl.bind("SUPER + V", function() hl.exec_cmd("hoarder picker") end)"#),
        "{module}"
    );
    assert!(
        module.contains(r#"hl.on("hyprland.start", function()"#),
        "{module}"
    );
    assert!(
        module.contains(
            "systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP \
             HYPRLAND_INSTANCE_SIGNATURE && systemctl --user start hoarder.service"
        ),
        "{module}"
    );
    assert!(!module.contains("bind ="), "{module}");
    assert!(!dir.join("hoarder.conf").exists());
    assert!(!dir.join("hyprland.conf").exists());
    // The main config is never edited; the require is offered instead.
    assert_eq!(
        std::fs::read_to_string(dir.join("hyprland.lua")).unwrap(),
        main_text
    );
    assert!(
        out.contains(&format!("Wrote {}", dir.join("hoarder.lua").display())),
        "{out}"
    );
    assert!(out.contains("    require(\"hoarder\")"), "{out}");
    assert!(!out.contains("source ="), "{out}");
}

#[test]
fn setup_hyprland_prints_lua_for_a_lua_config_without_apply() {
    let session = Session::start_default().unwrap();
    let dir = hypr_with(&session, "hyprland.lua", "");

    let out = ok(&hoarder(&session, &["setup", "hyprland"]));

    assert!(
        out.contains(r#"hl.bind("ALT + Print", function() hl.exec_cmd("hoarder pick") end)"#),
        "{out}"
    );
    assert!(
        out.contains(&dir.join("hoarder.lua").display().to_string()),
        "{out}"
    );
    assert!(!dir.join("hoarder.lua").exists());
}

#[test]
fn setup_hyprland_lua_stays_quiet_about_a_require_that_is_already_there() {
    let session = Session::start_default().unwrap();
    hypr_with(
        &session,
        "hyprland.lua",
        "require(\"modules.binds\")\nrequire 'hoarder'\n",
    );

    let out = ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));

    assert!(!out.contains("Add this line"), "{out}");
}

#[test]
fn setup_hyprland_lua_offers_the_require_when_it_is_commented_out() {
    let session = Session::start_default().unwrap();
    hypr_with(&session, "hyprland.lua", "-- require(\"hoarder\")\n");

    let out = ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));

    assert!(out.contains("Add this line"), "{out}");
    assert!(out.contains("    require(\"hoarder\")"), "{out}");
}

#[test]
fn setup_hyprland_keeps_hyprlang_for_a_conf_config() {
    let session = Session::start_default().unwrap();
    let dir = hypr_with(
        &session,
        "hyprland.conf",
        "monitor = , preferred, auto, 1\n",
    );

    let out = ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));

    let included = std::fs::read_to_string(dir.join("hoarder.conf")).unwrap();
    assert!(
        included.contains("bind = , Print, exec, hoarder shot"),
        "{included}"
    );
    assert!(
        included.contains("exec-once = systemctl --user import-environment"),
        "{included}"
    );
    assert!(!dir.join("hoarder.lua").exists());
    assert!(
        out.contains(&format!("source = {}", dir.join("hoarder.conf").display())),
        "{out}"
    );
}

/// What Hyprland itself makes of the written config, when it is installed:
/// `--verify-config` on the session's private HOME only (never the real
/// one), with no compositor started or reloaded.
fn hyprland_verifies(session: &Session, main: &std::path::Path) -> Option<Output> {
    let found = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join("Hyprland").is_file()));
    if !found {
        eprintln!("skipping: Hyprland is not installed");
        return None;
    }
    let runtime = session.runtime_dir().join("hyprland-verify");
    std::fs::create_dir_all(&runtime).unwrap();
    let home = session.runtime_dir().join("home");
    Some(
        std::process::Command::new("Hyprland")
            .arg("--verify-config")
            .arg("--config")
            .arg(main)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .current_dir(&runtime)
            .stdin(Stdio::null())
            .output()
            .expect("Hyprland should run"),
    )
}

fn assert_verified(out: Output) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("config ok"),
        "Hyprland rejected the config ({}):\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn hyprland_accepts_the_written_lua_module() {
    let session = Session::start_default().unwrap();
    let dir = hypr_with(&session, "hyprland.lua", "require(\"hoarder\")\n");
    ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));

    if let Some(out) = hyprland_verifies(&session, &dir.join("hyprland.lua")) {
        assert_verified(out);
    }
}

#[test]
fn setup_hyprland_follows_lua_when_both_configs_are_there() {
    let session = Session::start_default().unwrap();
    // Hyprland loads hyprland.lua whenever it exists (0.57 ignores
    // hyprland.conf), so a leftover .conf must not win.
    let dir = hypr_with(
        &session,
        "hyprland.conf",
        "monitor = , preferred, auto, 1\n",
    );
    std::fs::write(dir.join("hyprland.lua"), "").unwrap();

    let out = ok(&hoarder(&session, &["setup", "hyprland", "--apply"]));

    assert!(dir.join("hoarder.lua").is_file());
    assert!(!dir.join("hoarder.conf").exists());
    assert!(out.contains("    require(\"hoarder\")"), "{out}");
}
