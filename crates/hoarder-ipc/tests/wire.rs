//! The wire format is the contract (ADR-0004): every request and event this
//! crate knows, pinned to the JSON line that PLAN.md §11 and docs/ipc.md
//! promise.

use hoarder_ipc::{
    ClipSummary, ErrorCode, Event, EventLine, Kind, Op, Recording, Request, Selection, Sensitivity,
    State, encode, parse_request,
};
use serde_json::{Value, json};

fn wire<T: serde::Serialize>(message: &T) -> Value {
    let line = encode(message);
    assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
    serde_json::from_str(&line).unwrap()
}

fn request(op: Op) -> Value {
    wire(&Request { v: 1, id: 7, op })
}

#[test]
fn every_op_has_its_documented_line() {
    let cases = [
        (Op::Hello, json!({"op": "hello"})),
        (Op::Status, json!({"op": "status"})),
        (Op::Reload, json!({"op": "reload"})),
        (Op::Subscribe, json!({"op": "subscribe"})),
        (
            Op::List {
                limit: Some(10),
                concealed: true,
            },
            json!({"op": "list", "limit": 10, "concealed": true}),
        ),
        (
            Op::List {
                limit: None,
                concealed: false,
            },
            json!({"op": "list"}),
        ),
        (
            Op::Search {
                query: "hoard".into(),
                limit: None,
            },
            json!({"op": "search", "query": "hoard"}),
        ),
        (
            Op::Get {
                clip: 412,
                mime: Some("text/plain".into()),
            },
            json!({"op": "get", "clip": 412, "mime": "text/plain"}),
        ),
        (Op::Thumb { clip: 412 }, json!({"op": "thumb", "clip": 412})),
        (
            Op::Restore {
                clip: 412,
                plain: false,
            },
            json!({"op": "restore", "clip": 412}),
        ),
        (
            Op::Restore {
                clip: -3,
                plain: true,
            },
            json!({"op": "restore", "clip": -3, "plain": true}),
        ),
        (
            Op::Delete { clip: 412 },
            json!({"op": "delete", "clip": 412}),
        ),
        (Op::Pin { clip: 412 }, json!({"op": "pin", "clip": 412})),
        (Op::Unpin { clip: 412 }, json!({"op": "unpin", "clip": 412})),
        (
            Op::ClearHistory { pinned: true },
            json!({"op": "clear_history", "pinned": true}),
        ),
        (
            Op::Pause { secs: Some(60) },
            json!({"op": "pause", "secs": 60}),
        ),
        (Op::Resume, json!({"op": "resume"})),
        (
            Op::Clear {
                selection: Selection::Primary,
            },
            json!({"op": "clear", "selection": "primary"}),
        ),
        (
            Op::Clear {
                selection: Selection::Clipboard,
            },
            json!({"op": "clear"}),
        ),
        (
            Op::Offer {
                selection: Selection::Clipboard,
                types: vec!["text/plain".into()],
                sensitive: false,
                once: true,
            },
            json!({"op": "offer", "types": ["text/plain"], "sensitive": false, "once": true}),
        ),
        (
            Op::Paste {
                selection: Selection::Clipboard,
                mime: "text/plain".into(),
            },
            json!({"op": "paste", "mime": "text/plain"}),
        ),
    ];
    for (op, mut expected) in cases {
        expected["v"] = json!(1);
        expected["id"] = json!(7);
        assert_eq!(request(op.clone()), expected, "{op:?}");
        // And a Daemon reads back exactly what was sent.
        let parsed = parse_request(&expected.to_string()).unwrap();
        assert_eq!(parsed.op, op);
    }
}

#[test]
fn optional_request_fields_may_be_left_out() {
    let parsed = parse_request(r#"{"v":1,"id":1,"op":"clear_history"}"#).unwrap();
    assert_eq!(parsed.op, Op::ClearHistory { pinned: false });
    let parsed = parse_request(r#"{"v":1,"id":1,"op":"offer","types":["a/b"]}"#).unwrap();
    assert_eq!(
        parsed.op,
        Op::Offer {
            selection: Selection::Clipboard,
            types: vec!["a/b".into()],
            sensitive: false,
            once: false,
        }
    );
}

#[test]
fn unknown_request_fields_are_ignored() {
    let parsed = parse_request(r#"{"v":1,"id":1,"op":"status","from":"the future"}"#).unwrap();
    assert_eq!(parsed.op, Op::Status);
}

#[test]
fn broken_requests_get_the_documented_error_codes() {
    let code = |line: &str| match parse_request(line).unwrap_err().outcome {
        hoarder_ipc::Outcome::Err(f) => f.code,
        hoarder_ipc::Outcome::Ok(_) => panic!("{line} parsed"),
    };
    assert_eq!(code("not json"), ErrorCode::BadRequest);
    assert_eq!(code(r#"{"id":1,"op":"status"}"#), ErrorCode::BadRequest);
    assert_eq!(code(r#"{"v":1,"op":"status"}"#), ErrorCode::BadRequest);
    assert_eq!(code(r#"{"v":1,"id":1}"#), ErrorCode::BadRequest);
    assert_eq!(
        code(r#"{"v":2,"id":1,"op":"status"}"#),
        ErrorCode::UnsupportedVersion
    );
    assert_eq!(code(r#"{"v":1,"id":1,"op":"dance"}"#), ErrorCode::UnknownOp);
    assert_eq!(code(r#"{"v":1,"id":1,"op":"get"}"#), ErrorCode::BadRequest);
    // A known op with a bad value inside is a bad request, not an unknown op.
    assert_eq!(
        code(r#"{"v":1,"id":1,"op":"clear","selection":"secondary"}"#),
        ErrorCode::BadRequest
    );
}

fn clip() -> ClipSummary {
    ClipSummary {
        id: 412,
        kind: Kind::Text,
        preview: "hoard me".into(),
        app: Some("foot".into()),
        at: "2026-11-08T12:00:01Z".into(),
        created: "2026-11-08T12:00:00Z".into(),
        pinned: false,
        size: 8,
        types: vec!["text/plain".into()],
        sensitivity: Sensitivity::Normal,
        expires: None,
    }
}

fn clip_json() -> Value {
    json!({
        "id": 412, "kind": "text", "preview": "hoard me", "app": "foot",
        "at": "2026-11-08T12:00:01Z", "created": "2026-11-08T12:00:00Z",
        "pinned": false, "size": 8, "types": ["text/plain"],
    })
}

#[test]
fn a_concealed_clip_says_so_and_when_it_goes() {
    let concealed = ClipSummary {
        id: -3,
        preview: "••••••••".into(),
        sensitivity: Sensitivity::Concealed,
        expires: Some("2026-11-08T12:01:00Z".into()),
        ..clip()
    };
    let mut expected = clip_json();
    expected["id"] = json!(-3);
    expected["preview"] = json!("••••••••");
    expected["sensitivity"] = json!("concealed");
    expected["expires"] = json!("2026-11-08T12:01:00Z");
    assert_eq!(wire(&concealed), expected);
    assert_eq!(
        serde_json::from_value::<ClipSummary>(expected).unwrap(),
        concealed
    );
    // A History Clip leaves both out, and older lines without them decode.
    assert_eq!(wire(&clip()), clip_json());
    assert_eq!(
        serde_json::from_value::<ClipSummary>(clip_json()).unwrap(),
        clip()
    );
}

#[test]
fn every_event_has_its_documented_line() {
    let cases = [
        (
            Event::Selection {
                selection: Selection::Clipboard,
                types: Some(vec!["text/plain".into()]),
            },
            json!({"event": "selection", "selection": "clipboard", "types": ["text/plain"]}),
        ),
        (
            Event::Selection {
                selection: Selection::Primary,
                types: None,
            },
            json!({"event": "selection", "selection": "primary", "types": null}),
        ),
        (
            Event::ClipAdded { clip: clip() },
            json!({"event": "clip_added", "clip": clip_json()}),
        ),
        (
            Event::ClipRemoved { clip: 412 },
            json!({"event": "clip_removed", "clip": 412}),
        ),
        (
            Event::State(Recording {
                state: State::Paused,
                until: Some("2026-11-08T13:00:00Z".into()),
            }),
            json!({"event": "state", "state": "paused", "until": "2026-11-08T13:00:00Z"}),
        ),
        (
            Event::State(Recording {
                state: State::Active,
                until: None,
            }),
            json!({"event": "state", "state": "active"}),
        ),
    ];
    for (event, mut expected) in cases {
        expected["v"] = json!(1);
        let line = EventLine {
            v: 1,
            event: event.clone(),
        };
        assert_eq!(wire(&line), expected, "{event:?}");
        let back: EventLine = serde_json::from_value(expected).unwrap();
        assert_eq!(back.event, event);
    }
}

#[test]
fn events_newer_than_this_crate_are_unknown_not_errors() {
    let line: EventLine =
        serde_json::from_str(r#"{"v":1,"event":"weather","sky":"blue"}"#).unwrap();
    assert_eq!(line.event, Event::Unknown);
}

#[test]
fn newer_states_and_error_codes_decode_as_unknown() {
    let state: Recording = serde_json::from_str(r#"{"state":"hibernating"}"#).unwrap();
    assert_eq!(state.state, State::Unknown);
    let code: ErrorCode = serde_json::from_str(r#""on_fire""#).unwrap();
    assert_eq!(code, ErrorCode::Unknown);
}

#[test]
fn every_kind_keeps_its_wire_name_and_unknown_ones_read_as_other() {
    for (kind, name) in [
        (Kind::Text, "text"),
        (Kind::Image, "image"),
        (Kind::Files, "files"),
        (Kind::Other, "other"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
        assert_eq!(serde_json::from_value::<Kind>(json!(name)).unwrap(), kind);
    }
    let future = serde_json::from_value::<Kind>(json!("hologram")).unwrap();
    assert_eq!(future, Kind::Other);
}
