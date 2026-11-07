//! `hoarder shot` without `-o`: the Capture is saved to `screenshot.dir` under
//! the `screenshot.name` template, and flags override settings for one run.

use std::path::{Path, PathBuf};
use std::process::Output;

use hoarder_e2e::{OutputSpec, Session};

fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(args)
        .output()
        .expect("hoarder should run")
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn home(session: &Session) -> PathBuf {
    session.runtime_dir().join("home")
}

/// Every file under `dir`, sorted.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} should exist: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
}

fn png_size(path: &Path) -> (u32, u32) {
    let file = std::fs::File::open(path).unwrap();
    let reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("a valid PNG");
    let info = reader.info();
    (info.width, info.height)
}

fn file_name(path: &Path) -> &str {
    path.file_name().unwrap().to_str().unwrap()
}

/// `Hoarder_YYYY-MM-DD_HH-MM-SS.png`, checked character class by class.
fn is_default_name(name: &str) -> bool {
    let Some(stamp) = name
        .strip_prefix("Hoarder_")
        .and_then(|n| n.strip_suffix(".png"))
    else {
        return false;
    };
    let shape = "dddd-dd-dd_dd-dd-dd";
    stamp.len() == shape.len()
        && stamp.chars().zip(shape.chars()).all(|(c, s)| match s {
            'd' => c.is_ascii_digit(),
            s => c == s,
        })
}

#[test]
fn shot_saves_to_xdg_pictures_screenshots_by_default() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();

    let out = hoarder(&session, &["shot", "all"]);

    assert_ok(&out);
    let dir = home(&session).join("Pictures/Screenshots");
    let files = files_in(&dir);
    assert_eq!(files.len(), 1, "one Capture saved: {files:?}");
    assert!(
        is_default_name(file_name(&files[0])),
        "default name template: {:?}",
        files[0]
    );
    assert_eq!(png_size(&files[0]), (320, 240));
    // The saved path is printed so scripts can pick it up.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        files[0].to_str().unwrap()
    );
}

fn write_config(session: &Session, text: &str) {
    let dir = home(session).join(".config/hoarder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

#[test]
fn shot_follows_dir_and_name_template_from_the_config_file() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    write_config(
        &session,
        "[screenshot]\ndir = \"~/caps\"\nname = \"{target}{app}_{w}x{h}_{%Y}\"\n",
    );

    let out = hoarder(&session, &["shot", "all"]);

    assert_ok(&out);
    let files = files_in(&home(&session).join("caps"));
    let year = jiff::Zoned::now().year();
    // {app} is empty: All Outputs has no app.
    assert_eq!(
        files.iter().map(|f| file_name(f)).collect::<Vec<_>>(),
        [format!("all_320x240_{year}.png")]
    );
}

#[test]
fn a_second_capture_with_the_same_name_does_not_overwrite_the_first() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    write_config(&session, "[screenshot]\nname = \"same\"\n");

    assert_ok(&hoarder(&session, &["shot", "all"]));
    assert_ok(&hoarder(&session, &["shot", "all"]));

    let files = files_in(&home(&session).join("Pictures/Screenshots"));
    let names: Vec<_> = files.iter().map(|f| file_name(f)).collect();
    assert_eq!(names, ["same.png", "same_1.png"]);
}

#[test]
fn xdg_pictures_dir_comes_from_user_dirs_when_not_in_the_environment() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let config = home(&session).join(".config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("user-dirs.dirs"),
        "# written by xdg-user-dirs-update\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n",
    )
    .unwrap();

    assert_ok(&hoarder(&session, &["shot", "all"]));

    assert_eq!(
        files_in(&home(&session).join("Bilder/Screenshots")).len(),
        1
    );
}

#[test]
fn set_flag_overrides_a_setting_for_one_run_only() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    write_config(&session, "[screenshot]\nname = \"from-file\"\n");

    let out = hoarder(
        &session,
        &[
            "--set",
            "screenshot.name=from-flag",
            "--set",
            "screenshot.dir=~/once",
            "shot",
            "all",
        ],
    );
    assert_ok(&out);
    assert_ok(&hoarder(&session, &["shot", "all"]));

    let once = files_in(&home(&session).join("once"));
    assert_eq!(file_name(&once[0]), "from-flag.png");
    let normal = files_in(&home(&session).join("Pictures/Screenshots"));
    assert_eq!(file_name(&normal[0]), "from-file.png");
}

#[test]
fn save_false_saves_nothing_and_o_writes_only_where_asked() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();

    assert_ok(&hoarder(
        &session,
        &["--set", "screenshot.save=false", "shot", "all"],
    ));
    let path = home(&session).join("explicit.png");
    assert_ok(&hoarder(
        &session,
        &["shot", "all", "-o", path.to_str().unwrap()],
    ));

    assert!(!home(&session).join("Pictures").exists(), "nothing saved");
    assert_eq!(png_size(&path), (320, 240));
}

#[test]
fn a_broken_config_warns_with_its_position_but_still_captures() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    write_config(&session, "[screenshot]\nformat = \"gif\"\n");

    let out = hoarder(&session, &["shot", "all"]);

    assert_ok(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("config.toml:2:10:"), "position: {stderr}");
    assert_eq!(
        files_in(&home(&session).join("Pictures/Screenshots")).len(),
        1
    );
}

#[test]
fn a_bad_set_flag_is_a_usage_error() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();

    let out = hoarder(&session, &["--set", "screenshot.nope=1", "shot", "all"]);

    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("screenshot.nope"));
}
