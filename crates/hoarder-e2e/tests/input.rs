//! The harness injects pointer and keyboard input that lands on whatever is
//! under the pointer / focused, exactly as a real device's would.

use hoarder_e2e::InputEvent::{Key, PointerButton, PointerMotion};
use hoarder_e2e::{InputEvent, Session, keys};

#[test]
fn injected_pointer_and_keys_reach_the_focused_window() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    app.open_window("hoarder-e2e.probe").unwrap();
    let input = session.input();

    input
        .move_to(session.output("HEADLESS-1"), 100.0, 50.0)
        .unwrap();
    input.click(keys::BTN_LEFT).unwrap();
    input.tap(keys::KEY_A).unwrap();
    input.chord(&[keys::KEY_LEFTCTRL, keys::KEY_V]).unwrap();

    let events: Vec<InputEvent> = app
        .take_input_events()
        .unwrap()
        .into_iter()
        .filter(|e| matches!(e, PointerMotion { .. } | PointerButton { .. } | Key { .. }))
        .collect();
    assert_eq!(
        events,
        vec![
            PointerMotion { x: 100.0, y: 50.0 },
            PointerButton {
                button: 0x110,
                pressed: true
            },
            PointerButton {
                button: 0x110,
                pressed: false
            },
            Key {
                key: 30,
                pressed: true
            },
            Key {
                key: 30,
                pressed: false
            },
            Key {
                key: 29,
                pressed: true
            },
            Key {
                key: 47,
                pressed: true
            },
            Key {
                key: 47,
                pressed: false
            },
            Key {
                key: 29,
                pressed: false
            },
        ]
    );
}

#[test]
fn modifiers_follow_injected_keys() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    app.open_window("hoarder-e2e.probe").unwrap();
    let input = session.input();

    input.key_down(keys::KEY_LEFTSHIFT).unwrap();

    let events = app.take_input_events().unwrap();
    assert!(
        events.contains(&InputEvent::Modifiers { depressed: 1 }),
        "Shift should show up as a depressed modifier: {events:?}"
    );
}
