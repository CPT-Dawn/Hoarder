//! Window geometry providers: what they make of their compositor's IPC
//! replies. The Sway fixture is a real `get_tree` from headless Sway 1.12;
//! the Hyprland ones follow `j/clients` and `j/monitors` from Hyprland
//! 0.57.1, with titles made up.

use hoarder_wayland::LogicalRect;
use hoarder_wayland::windows::{HyprlandIpc, SwayIpc, Window};

fn rect(x: i32, y: i32, width: u32, height: u32) -> LogicalRect {
    LogicalRect {
        x,
        y,
        width,
        height,
    }
}

fn summary(windows: &[Window]) -> Vec<(&str, LogicalRect)> {
    windows.iter().map(|w| (w.title.as_str(), w.rect)).collect()
}

/// HEADLESS-2 (logical x 640) shows tiled `probe.d`; HEADLESS-1 shows tiled
/// `probe.b` under floating `probe.a` (4 px border); `probe.c` is in the
/// scratchpad.
#[test]
fn sway_lists_visible_windows_topmost_first_by_their_content() {
    let windows =
        SwayIpc::parse_tree(include_bytes!("fixtures/sway-get-tree.json")).expect("parses");
    assert_eq!(
        summary(&windows),
        vec![
            ("probe.a", rect(104, 64, 192, 112)),
            ("probe.b", rect(0, 0, 640, 480)),
            ("probe.d", rect(640, 0, 400, 300)),
        ]
    );
    assert_eq!(windows[0].app_id, "probe.a");
}

#[test]
fn sway_puts_fullscreen_windows_on_top_and_reads_xwayland_classes() {
    let tree = br#"{
      "type": "root", "name": "root", "rect": {"x":0,"y":0,"width":800,"height":600},
      "nodes": [{
        "type": "output", "name": "HEADLESS-1", "rect": {"x":0,"y":0,"width":800,"height":600},
        "nodes": [{
          "type": "workspace", "name": "1", "rect": {"x":0,"y":0,"width":800,"height":600},
          "nodes": [
            {"type": "con", "name": "fullscreen game", "app_id": null,
             "window_properties": {"class": "Game", "title": "fullscreen game"},
             "rect": {"x":0,"y":0,"width":800,"height":600},
             "window_rect": {"x":0,"y":0,"width":800,"height":600},
             "visible": true, "fullscreen_mode": 1, "nodes": [], "floating_nodes": []},
            {"type": "con", "name": "hidden tab", "app_id": "foot",
             "rect": {"x":0,"y":0,"width":800,"height":600},
             "window_rect": {"x":0,"y":0,"width":800,"height":600},
             "visible": false, "fullscreen_mode": 0, "nodes": [], "floating_nodes": []}
          ],
          "floating_nodes": [
            {"type": "floating_con", "name": "dialog", "app_id": "zenity",
             "rect": {"x":100,"y":100,"width":200,"height":100},
             "deco_rect": {"x":0,"y":-24,"width":200,"height":24},
             "window_rect": {"x":2,"y":0,"width":196,"height":98},
             "visible": true, "fullscreen_mode": 0, "nodes": [], "floating_nodes": []}
          ]
        }]
      }]
    }"#;
    let windows = SwayIpc::parse_tree(tree).expect("parses");
    assert_eq!(
        summary(&windows),
        vec![
            ("fullscreen game", rect(0, 0, 800, 600)),
            ("dialog", rect(102, 100, 196, 98)),
        ]
    );
    assert_eq!(windows[0].app_id, "Game");
}

#[test]
fn hyprland_lists_windows_on_shown_workspaces_in_approximate_stacking_order() {
    let windows = HyprlandIpc::parse(
        include_bytes!("fixtures/hyprland-clients.json"),
        include_bytes!("fixtures/hyprland-monitors.json"),
    )
    .expect("parses");
    assert_eq!(
        summary(&windows),
        vec![
            // The open special workspace is drawn over everything.
            ("scratchpad terminal", rect(1800, 100, 800, 600)),
            // Floating over tiled.
            ("floating mixer", rect(2000, 200, 600, 400)),
            // Tiled, most recently focused first.
            ("tiled browser", rect(1541, 5, 950, 1070)),
            ("laptop terminal", rect(2, 34, 1532, 924)),
        ]
    );
    assert_eq!(windows[1].app_id, "pavucontrol");
}

#[test]
fn hyprland_before_0_42_had_a_boolean_fullscreen() {
    let clients = br#"[
      {"mapped": true, "hidden": false, "at": [0, 0], "size": [100, 100],
       "workspace": {"id": 1, "name": "1"}, "floating": false, "pinned": false,
       "fullscreen": false, "focusHistoryID": 0, "class": "a", "title": "tiled"},
      {"mapped": true, "hidden": false, "at": [0, 0], "size": [1920, 1080],
       "workspace": {"id": 1, "name": "1"}, "floating": false, "pinned": false,
       "fullscreen": true, "focusHistoryID": 1, "class": "b", "title": "fullscreen"}
    ]"#;
    let monitors = br#"[{"activeWorkspace": {"id": 1, "name": "1"},
                         "specialWorkspace": {"id": 0, "name": ""}}]"#;
    let windows = HyprlandIpc::parse(clients, monitors).expect("parses");
    assert_eq!(
        windows.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(),
        ["fullscreen", "tiled"]
    );
}

/// For `shot window --list`: every window, how to name its toplevel, and
/// the scale it is drawn at.
#[test]
fn sway_lists_every_window_with_its_toplevel_and_scale() {
    let windows =
        SwayIpc::parse_tree_all(include_bytes!("fixtures/sway-get-tree.json")).expect("parses");
    let seen: Vec<(&str, Option<&str>, Option<f64>, bool)> = windows
        .iter()
        .map(|w| {
            (
                w.title.as_str(),
                w.toplevel.as_deref(),
                w.scale,
                w.may_be_stale,
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                "probe.a",
                Some("55ee991b33c1f14817db9dc84259ed9a"),
                Some(1.0),
                false
            ),
            (
                "probe.b",
                Some("7d0d69c55b84ce8999c31b9b14e15c77"),
                Some(1.0),
                false
            ),
            (
                "probe.d",
                Some("fb9b961e748a321c3b95126d191f402d"),
                Some(2.0),
                false
            ),
            // The scratchpad: on no output.
            (
                "probe.c",
                Some("21e255a4e871bd847dda9d12cd242b3e"),
                None,
                false
            ),
        ]
    );
}

/// Hyprland does not redraw windows on workspaces no monitor shows: their
/// own buffers may be stale.
#[test]
fn hyprland_lists_every_mapped_window_marking_hidden_ones_maybe_stale() {
    let windows = HyprlandIpc::parse_all(
        include_bytes!("fixtures/hyprland-clients.json"),
        include_bytes!("fixtures/hyprland-monitors.json"),
    )
    .expect("parses");
    let seen: Vec<(&str, Option<u64>, Option<f64>, bool)> = windows
        .iter()
        .map(|w| (w.title.as_str(), w.address, w.scale, w.may_be_stale))
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                "scratchpad terminal",
                Some(0x557648e80000),
                Some(1.0),
                false
            ),
            ("floating mixer", Some(0x557648e60000), Some(1.0), false),
            ("tiled browser", Some(0x557648e50bd0), Some(1.0), false),
            (
                "laptop terminal",
                Some(0x55764847fa40),
                Some(1.6666666),
                false
            ),
            (
                "on a hidden workspace",
                Some(0x557648e70000),
                Some(1.0),
                true
            ),
            ("hidden group member", Some(0x557648e90000), Some(1.0), true),
        ]
    );
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(SwayIpc::parse_tree(b"not json").is_err());
    assert!(HyprlandIpc::parse(b"[]", b"{").is_err());
}
