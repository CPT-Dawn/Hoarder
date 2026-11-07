//! The `hoarder` binary, run as a black box inside a headless Sway session.

use hoarder_e2e::Session;

#[test]
fn hoarder_reports_its_version_inside_headless_sway() {
    let session = Session::start_default().expect("headless Sway should start");

    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .arg("--version")
        .output()
        .expect("hoarder should run");

    assert!(out.status.success(), "hoarder --version failed: {out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("hoarder {}\n", env!("CARGO_PKG_VERSION"))
    );
}
