//! The helper client sets and serves Selections like an app would, and they
//! vanish when it exits. `wl-paste` (wl-clipboard) is the independent oracle.

use hoarder_e2e::{Selection, Session};

fn wl_paste(session: &Session, args: &[&str]) -> Option<Vec<u8>> {
    let out = session
        .command("wl-paste")
        .args(args)
        .output()
        .expect("wl-paste should run");
    out.status.success().then_some(out.stdout)
}

fn types(session: &Session, primary: bool) -> Option<String> {
    let args: &[&str] = if primary {
        &["--primary", "--list-types"]
    } else {
        &["--list-types"]
    };
    wl_paste(session, args).map(|o| String::from_utf8(o).unwrap())
}

#[test]
fn serves_a_clipboard_selection_until_told_to_exit() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();

    app.set_selection(
        Selection::Clipboard,
        &[
            ("text/plain;charset=utf-8", b"hoard me".as_slice()),
            ("x-kde-passwordManagerHint", b"secret".as_slice()),
        ],
    )
    .unwrap();

    assert_eq!(
        types(&session, false).as_deref(),
        Some("text/plain;charset=utf-8\nx-kde-passwordManagerHint\n")
    );
    assert_eq!(
        wl_paste(
            &session,
            &["--no-newline", "--type", "text/plain;charset=utf-8"]
        )
        .as_deref(),
        Some(b"hoard me".as_slice())
    );
    assert_eq!(
        wl_paste(
            &session,
            &["--no-newline", "--type", "x-kde-passwordManagerHint"]
        )
        .as_deref(),
        Some(b"secret".as_slice())
    );

    app.exit();

    assert_eq!(
        types(&session, false),
        None,
        "the Selection should vanish with its app"
    );
}

#[test]
fn serves_a_primary_selection_and_large_payloads() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    let big: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();

    app.set_selection(Selection::Primary, &[("image/png", big.as_slice())])
        .unwrap();

    assert_eq!(types(&session, true).as_deref(), Some("image/png\n"));
    assert_eq!(
        types(&session, false),
        None,
        "only the Primary Selection was set"
    );
    assert_eq!(
        wl_paste(&session, &["--primary", "--type", "image/png"]),
        Some(big)
    );
}
